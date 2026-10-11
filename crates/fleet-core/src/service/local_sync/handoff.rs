//! Phases 2 and 3 of the local workspace
//! (`docs/superpowers/specs/2026-10-07-local-workspace-handoff-design.md`):
//! what changed on which side, the diff, commit and discard, conflict
//! compare / keep-both, and the prompts that hand the work to the agent and
//! back. The prompts are composed here and sent by the desktop command
//! (through `sessions::send_prompt`, which needs the concrete SSH client);
//! [`finish_ask`] then clears what the agent was handed.

use super::{git, local, now_secs, LocalSync, MAX_FILE_BYTES};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::repo_read::{ChangedFile, FileDiff};
use crate::store::{LocalActivityRow, LocalWorkspaceRow, SessionRow, Store};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A prompt lists at most this many files, then "and N more".
const MAX_LISTED: usize = 200;
/// A commit or discard names at most this many paths.
const MAX_PATHS: usize = 5000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalWorkspacePathArgs {
    pub id: i64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalWorkspacePathsArgs {
    pub id: i64,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitLocalWorkspaceArgs {
    pub id: i64,
    pub message: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DismissLocalActivityArgs {
    pub id: i64,
    /// `local` or `remote`; both when absent.
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskAiArgs {
    pub id: i64,
    /// `explain`, `review`, `continue`, `tests`, `commit`, `merge`,
    /// `resolve` (needs `paths` = the one conflicting path) or `custom`
    /// (needs `question`).
    pub intent: String,
    /// The question for `custom`; for any other intent an optional note
    /// appended to the ask.
    #[serde(default)]
    pub question: Option<String>,
    /// The files to talk about; the link's unhanded local changes when
    /// absent.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetLocalDriverArgs {
    pub id: i64,
    /// `shared`, `developer` or `agent`.
    pub driver: String,
}

/// One uncommitted change in the worktree, and which side made it when the
/// sync carried it (`None`: before the link, or made on the host and not
/// carried yet).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalChange {
    #[serde(flatten)]
    pub file: ChangedFile,
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalChanges {
    /// The branch checked out in the worktree (`HEAD` when detached).
    pub branch: String,
    /// `git status` of the worktree on the host, without what the link
    /// leaves out.
    pub files: Vec<LocalChange>,
    /// The activity log: carried changes nobody has looked at yet.
    pub activity: Vec<LocalActivityRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalCommit {
    pub commit: String,
}

/// A prompt to deliver: which session, what to say, and which local
/// activity rows to clear once it was delivered.
#[derive(Debug, Clone, PartialEq)]
pub struct AskPlan {
    pub session_id: i64,
    pub host_alias: String,
    pub tmux_name: String,
    pub prompt: String,
    pub clear: Vec<String>,
}

fn row(store: &Mutex<Store>, id: i64) -> Result<LocalWorkspaceRow, IpcError> {
    lock(store)?
        .local_workspace(id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no local workspace {id}")))
}

fn check_paths(paths: &[String]) -> Result<(), IpcError> {
    if paths.is_empty() {
        return Err(IpcError::new(codes::E_INVALID, "choose at least one file"));
    }
    if paths.len() > MAX_PATHS {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("at most {MAX_PATHS} files at once"),
        ));
    }
    for p in paths {
        crate::validate::repo_rel_path(p)?;
    }
    Ok(())
}

/// The worktree's uncommitted changes, after a pass (so an edit made in the
/// folder a moment ago is in it), each with the side that made it.
pub async fn changes(engine: &Arc<LocalSync>, id: i64) -> Result<LocalChanges, IpcError> {
    let link = row(&engine.store, id)?;
    if !link.paused {
        engine.sync_now(id).await?;
    }
    let (branch, files) =
        git::status(engine.ssh.as_ref(), &link.host_alias, &link.remote_path).await?;
    let activity = lock(&engine.store)?.local_workspace_activity(id)?;
    let origin: HashMap<&str, &str> = activity
        .iter()
        .map(|a| (a.path.as_str(), a.origin.as_str()))
        .collect();
    // What the link leaves out (build output, IDE metadata) is nobody's
    // edit to review, even where the project's .gitignore lets git see it.
    let ex = super::excludes::Excludes::new(&link.excludes)?;
    let files = files
        .into_iter()
        .filter(|f| !ex.excluded(&f.path, false))
        .map(|f| LocalChange {
            origin: origin.get(f.path.as_str()).map(|o| o.to_string()),
            file: f,
        })
        .collect();
    Ok(LocalChanges {
        branch,
        files,
        activity,
    })
}

/// One file's change against `HEAD` in the worktree.
pub async fn diff(
    engine: &Arc<LocalSync>,
    args: LocalWorkspacePathArgs,
) -> Result<FileDiff, IpcError> {
    crate::validate::repo_rel_path(&args.path)?;
    let link = row(&engine.store, args.id)?;
    git::diff(
        engine.ssh.as_ref(),
        &link.host_alias,
        &link.remote_path,
        &args.path,
    )
    .await
}

/// Commit exactly `paths` in the worktree (after a pass, so the folder's
/// latest edits are in). Their activity rows go: they are committed.
pub async fn commit(
    engine: &Arc<LocalSync>,
    args: CommitLocalWorkspaceArgs,
) -> Result<LocalCommit, IpcError> {
    check_paths(&args.paths)?;
    let message = args.message.trim();
    if message.is_empty() {
        return Err(IpcError::new(codes::E_INVALID, "write a commit message"));
    }
    let link = row(&engine.store, args.id)?;
    if !link.paused {
        engine.sync_now(args.id).await?;
    }
    let commit = git::commit(
        engine.ssh.as_ref(),
        &link.host_alias,
        &link.remote_path,
        message,
        &args.paths,
    )
    .await?;
    lock(&engine.store)?.clear_local_workspace_activity(args.id, None, Some(&args.paths))?;
    Ok(LocalCommit { commit })
}

/// Put `paths` back to `HEAD` in the worktree; the next pass carries that to
/// the folder under the usual guards. Refused while paused: the folder
/// would keep the discarded edit and push it back on resume.
pub async fn discard(
    engine: &Arc<LocalSync>,
    args: LocalWorkspacePathsArgs,
) -> Result<LocalWorkspaceRow, IpcError> {
    check_paths(&args.paths)?;
    let link = row(&engine.store, args.id)?;
    if link.paused {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            "resume sync first: a paused folder would bring the discarded edits back",
        ));
    }
    // The folder's latest edits first, so what is discarded is what was shown.
    engine.sync_now(args.id).await?;
    git::discard(
        engine.ssh.as_ref(),
        &link.host_alias,
        &link.remote_path,
        &args.paths,
    )
    .await?;
    lock(&engine.store)?.clear_local_workspace_activity(args.id, None, Some(&args.paths))?;
    let after = engine.sync_now(args.id).await?;
    // The pass that carried the discard to the folder logged it as the
    // host's change; it is nobody's.
    lock(&engine.store)?.clear_local_workspace_activity(args.id, None, Some(&args.paths))?;
    row(&engine.store, after.id)
}

/// Forget the activity log (one side, or both).
pub fn dismiss(
    engine: &Arc<LocalSync>,
    args: DismissLocalActivityArgs,
) -> Result<LocalWorkspaceRow, IpcError> {
    lock(&engine.store)?.clear_local_workspace_activity(args.id, args.origin.as_deref(), None)?;
    row(&engine.store, args.id)
}

/// The open conflict on `path`, or `E_NOTFOUND`.
fn conflict_of(link: &LocalWorkspaceRow, path: &str) -> Result<(), IpcError> {
    if link.conflicts.iter().any(|c| c.path == path) {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("no open conflict on {path}"),
        ))
    }
}

/// The local file's bytes, `None` when it is not there.
fn read_local(root: &str, path: &str) -> Result<Option<Vec<u8>>, IpcError> {
    let abs = local::abs(Path::new(root), path);
    match std::fs::symlink_metadata(&abs) {
        Ok(m) if m.is_file() => {
            if m.len() > MAX_FILE_BYTES {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("{path} is larger than the sync carries"),
                ));
            }
            std::fs::read(&abs)
                .map(Some)
                .map_err(|e| IpcError::new(codes::E_IO, format!("read {path}: {e}")))
        }
        Ok(_) => Err(IpcError::new(
            codes::E_INVALID,
            format!("{path} is not a regular file here"),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(IpcError::new(codes::E_IO, format!("read {path}: {e}"))),
    }
}

/// A conflicting file as a diff from the local version to the host's.
pub async fn compare_conflict(
    engine: &Arc<LocalSync>,
    args: LocalWorkspacePathArgs,
) -> Result<FileDiff, IpcError> {
    crate::validate::repo_rel_path(&args.path)?;
    let link = row(&engine.store, args.id)?;
    conflict_of(&link, &args.path)?;
    let bytes = {
        let (root, path) = (link.local_path.clone(), args.path.clone());
        tokio::task::spawn_blocking(move || read_local(&root, &path))
            .await
            .map_err(super::join_err)??
    };
    git::compare(
        engine.ssh.as_ref(),
        &link.host_alias,
        &link.remote_path,
        &args.path,
        bytes,
    )
    .await
}

/// The name "keep both" saves the local version under.
pub fn local_copy_name(path: &str) -> String {
    format!("{path}.local-copy")
}

/// Keep both versions of a conflicting file: copy the local one to
/// `<path>.local-copy` (a new file the sync then carries), and resolve the
/// conflict to the host's version.
pub async fn keep_both(
    engine: &Arc<LocalSync>,
    args: LocalWorkspacePathArgs,
) -> Result<LocalWorkspaceRow, IpcError> {
    crate::validate::repo_rel_path(&args.path)?;
    let link = row(&engine.store, args.id)?;
    conflict_of(&link, &args.path)?;
    let copy = local_copy_name(&args.path);
    {
        let (root, path, copy) = (link.local_path.clone(), args.path.clone(), copy.clone());
        tokio::task::spawn_blocking(move || -> Result<(), IpcError> {
            let Some(bytes) = read_local(&root, &path)? else {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("{path} was deleted here, so there is no local version to keep"),
                ));
            };
            let r = Path::new(&root);
            if local::exists(r, &copy) {
                return Err(IpcError::new(
                    codes::E_CONFLICT,
                    format!("{copy} already exists; move it away first"),
                ));
            }
            match local::write_guarded(r, &copy, &bytes, local::is_executable(r, &path), None)? {
                local::Guarded::Done(_) => Ok(()),
                local::Guarded::Moved => Err(IpcError::new(
                    codes::E_CONFLICT,
                    format!("{copy} appeared meanwhile; move it away first"),
                )),
            }
        })
        .await
        .map_err(super::join_err)??;
    }
    lock(&engine.store)?.set_local_conflict_resolution(args.id, &args.path, "remote")?;
    engine.sync_now(args.id).await
}

// ---------------------------------------------------------------------------
// Prompts.
// ---------------------------------------------------------------------------

pub const ASK_INTENTS: [&str; 8] = [
    "explain", "review", "continue", "tests", "commit", "merge", "resolve", "custom",
];

/// The sessions a desktop paired with a hub asks about: the hub's rows, and
/// the hub's id of the link's project (owner/repo), since neither is in this
/// machine's database.
#[derive(Debug, Clone, Default)]
pub struct HubSessions {
    pub sessions: Vec<SessionRow>,
    pub project_id: Option<i64>,
}

/// The live work session on the link's worktree: the one that made the link
/// when it is still there, else the most recently active one.
pub fn target_session(store: &Store, link: &LocalWorkspaceRow) -> Result<SessionRow, IpcError> {
    pick_target(
        store.list_sessions_for_host(&link.host_alias)?,
        link,
        link.project_id,
    )
}

/// [`target_session`] among `sessions`, with `pid` the project's id there.
pub fn pick_target(
    sessions: Vec<SessionRow>,
    link: &LocalWorkspaceRow,
    pid: Option<i64>,
) -> Result<SessionRow, IpcError> {
    let alive: Vec<SessionRow> = sessions
        .into_iter()
        .filter(|s| {
            s.status != "ghost"
                && s.kind == "work"
                && s.host_alias == link.host_alias
                && pid.is_some()
                && s.project_id == pid
                && s.worktree_key
                    .as_deref()
                    .filter(|k| !k.is_empty())
                    .unwrap_or("main")
                    == link.worktree_key
        })
        .collect();
    if let Some(s) = link
        .session_id
        .and_then(|id| alive.iter().find(|s| s.id == id))
    {
        return Ok(s.clone());
    }
    alive.into_iter().next().ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!(
                "no session runs on this worktree ({}) now; start one on it first",
                link.worktree_key
            ),
        )
    })
}

fn find_target(
    store: &Store,
    link: &LocalWorkspaceRow,
    hub: Option<&HubSessions>,
) -> Result<SessionRow, IpcError> {
    match hub {
        Some(h) => pick_target(h.sessions.clone(), link, h.project_id),
        None => target_session(store, link),
    }
}

fn change_letter(change: &str) -> &'static str {
    match change {
        "added" => "A",
        "deleted" => "D",
        _ => "M",
    }
}

/// The file list a prompt carries: `M path` lines, at most [`MAX_LISTED`].
fn file_list(files: &[(String, String)]) -> String {
    let mut out = String::new();
    for (path, change) in files.iter().take(MAX_LISTED) {
        out.push_str(&format!("- {} {path}\n", change_letter(change)));
    }
    if files.len() > MAX_LISTED {
        out.push_str(&format!(
            "- … and {} more (see `git status`)\n",
            files.len() - MAX_LISTED
        ));
    }
    out
}

/// What the developer changed locally, as the prompt describes it.
fn changes_paragraph(link: &LocalWorkspaceRow, files: &[(String, String)]) -> String {
    let lead = format!(
        "The developer works on this worktree in a folder on their machine ({}), kept in \
         sync with it both ways. ",
        link.local_path
    );
    if files.is_empty() {
        format!(
            "{lead}Their edits are among the uncommitted changes here: see `git status` and \
             `git diff`.\n"
        )
    } else {
        format!(
            "{lead}They changed these files locally; the changes are synced here and not \
             committed:\n{}Run `git diff` on them (and `git status`) to see the changes.\n",
            file_list(files)
        )
    }
}

/// The prompt for `intent`. `files` are (path, change) pairs. `question` is
/// the whole ask for "custom"; for any other intent it is an optional note
/// the person adds ("and check the error paths"), appended after the ask.
pub fn compose(
    link: &LocalWorkspaceRow,
    intent: &str,
    question: Option<&str>,
    files: &[(String, String)],
) -> Result<String, IpcError> {
    let base = compose_intent(link, intent, question, files)?;
    let note = question.map(str::trim).filter(|q| !q.is_empty());
    Ok(match note {
        Some(q) if intent != "custom" => format!(
            "{base}
Also: {q}"
        ),
        _ => base,
    })
}

fn compose_intent(
    link: &LocalWorkspaceRow,
    intent: &str,
    question: Option<&str>,
    files: &[(String, String)],
) -> Result<String, IpcError> {
    let ask = match intent {
        "explain" => {
            "Explain what these changes do and why they were likely made. Do not edit files."
        }
        "review" => {
            "Review these changes: point out bugs, risks, and anything missing (tests, docs, \
             edge cases). Do not edit files unless I ask."
        }
        "continue" => {
            "Continue the task from these changes: build on them, finish what is incomplete, \
             and keep the developer's edits unless one is clearly a mistake (then say so)."
        }
        "tests" => "Write or update tests that cover these changes, and run them.",
        "commit" => {
            "Commit these changes: write a commit message that says what they do, and commit \
             exactly these files (nothing else). Do not push."
        }
        "merge" => {
            return Ok(format!(
                "Get this worktree's branch ready to merge: merge its base branch into it, \
                 resolve any conflicts, run the tests, and tell me whether it now merges \
                 cleanly. Do not push and do not change the base branch.\n(The developer also \
                 has this worktree open locally at {}; it stays in sync.)",
                link.local_path
            ))
        }
        "resolve" => {
            let [(path, _)] = files else {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "resolve names exactly one conflicting file",
                ));
            };
            return Ok(format!(
                "The file `{path}` was changed both here and in the developer's local copy of \
                 this worktree ({}), and the sync stopped on it. Their version is at \
                 `{path}.fleet-local` (do not commit that file). Merge the two into `{path}`, \
                 keeping what each side meant, then delete `{path}.fleet-local` and tell me \
                 what you kept from each.",
                link.local_path
            ));
        }
        "custom" => {
            let q = question
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "write the question to ask"))?;
            return Ok(format!("{}\n{q}", changes_paragraph(link, files)));
        }
        other => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("unknown intent {other:?}"),
            ))
        }
    };
    Ok(format!("{}\n{ask}", changes_paragraph(link, files)))
}

/// The local activity rows as (path, change), newest first, limited to
/// `paths` when given.
fn local_files(
    store: &Store,
    id: i64,
    paths: Option<&[String]>,
) -> Result<Vec<(String, String)>, IpcError> {
    let rows = store.local_workspace_activity(id)?;
    let by_path: HashMap<&str, &LocalActivityRow> =
        rows.iter().map(|a| (a.path.as_str(), a)).collect();
    Ok(match paths {
        Some(paths) => paths
            .iter()
            .map(|p| {
                let change = by_path
                    .get(p.as_str())
                    .map(|a| a.change.clone())
                    .unwrap_or_else(|| "modified".into());
                (p.clone(), change)
            })
            .collect(),
        None => rows
            .iter()
            .filter(|a| a.origin == "local")
            .map(|a| (a.path.clone(), a.change.clone()))
            .collect(),
    })
}

/// Everything "Ask AI" needs, without sending it: the target session, the
/// prompt, and which local activity rows the agent is handed. For
/// `resolve`, the local version of the file is first put next to the
/// host's as `<path>.fleet-local`.
pub async fn prepare_ask(
    engine: &Arc<LocalSync>,
    args: AskAiArgs,
    hub: Option<&HubSessions>,
) -> Result<AskPlan, IpcError> {
    if !ASK_INTENTS.contains(&args.intent.as_str()) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("unknown intent {:?}", args.intent),
        ));
    }
    if let Some(p) = &args.paths {
        if !p.is_empty() {
            check_paths(p)?;
        }
    }
    let link = row(&engine.store, args.id)?;
    // The folder's latest edits first, so the agent sees what the person saw.
    if !link.paused {
        engine.sync_now(args.id).await?;
    }
    let (session, files) = {
        let s = lock(&engine.store)?;
        let session = find_target(&s, &link, hub)?;
        let paths = args.paths.as_deref().filter(|p| !p.is_empty());
        (session, local_files(&s, args.id, paths)?)
    };
    if args.intent == "resolve" {
        let [(path, _)] = files.as_slice() else {
            return Err(IpcError::new(
                codes::E_INVALID,
                "resolve names exactly one conflicting file",
            ));
        };
        conflict_of(&link, path)?;
        let bytes = {
            let (root, p) = (link.local_path.clone(), path.clone());
            tokio::task::spawn_blocking(move || read_local(&root, &p))
                .await
                .map_err(super::join_err)??
        };
        let bytes = bytes.ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                format!("{path} was deleted here: pick Keep local or Keep remote instead"),
            )
        })?;
        git::stage_local_copy(
            engine.ssh.as_ref(),
            &link.host_alias,
            &link.remote_path,
            path,
            bytes,
        )
        .await?;
    }
    let prompt = compose(&link, &args.intent, args.question.as_deref(), &files)?;
    let clear = match args.intent.as_str() {
        // Handing the agent the changes is what clears them; asking it to
        // only explain or review them leaves them for the person too.
        "continue" | "tests" | "commit" => files.into_iter().map(|(p, _)| p).collect(),
        _ => Vec::new(),
    };
    Ok(AskPlan {
        session_id: session.id,
        host_alias: session.host_alias,
        tmux_name: session.tmux_name,
        prompt,
        clear,
    })
}

/// After the prompt was delivered: the agent has those changes now.
pub fn finish_ask(
    engine: &Arc<LocalSync>,
    id: i64,
    clear: &[String],
) -> Result<LocalWorkspaceRow, IpcError> {
    if !clear.is_empty() {
        lock(&engine.store)?.clear_local_workspace_activity(id, Some("local"), Some(clear))?;
    }
    row(&engine.store, id)
}

/// Who drives the worktree, and what to tell the agent about it: taking
/// over asks it to keep its hands off; handing back gives it the
/// developer's changes and asks it to continue. `shared` says nothing.
/// The driver is set by [`finish_driver`] once the message (if any) was
/// delivered.
pub async fn prepare_driver(
    engine: &Arc<LocalSync>,
    args: &SetLocalDriverArgs,
    hub: Option<&HubSessions>,
) -> Result<Option<AskPlan>, IpcError> {
    if !crate::store::LOCAL_WORKSPACE_DRIVERS.contains(&args.driver.as_str()) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "driver must be shared, developer or agent, not {:?}",
                args.driver
            ),
        ));
    }
    let link = row(&engine.store, args.id)?;
    match args.driver.as_str() {
        "developer" => {
            let found = {
                let s = lock(&engine.store)?;
                find_target(&s, &link, hub)
            };
            let session = match found {
                Ok(s) => s,
                // Nobody runs on the worktree: nothing to keep off it.
                Err(e) if e.code == codes::E_NOTFOUND => return Ok(None),
                Err(e) => return Err(e),
            };
            Ok(Some(AskPlan {
                session_id: session.id,
                host_alias: session.host_alias,
                tmux_name: session.tmux_name,
                prompt: format!(
                    "The developer is taking over this worktree and editing it in their local \
                     copy ({}), which stays in sync with it. Do not change files here until \
                     they hand it back; you can still answer questions and read the code.",
                    link.local_path
                ),
                clear: Vec::new(),
            }))
        }
        "agent" => {
            let plan = prepare_ask(
                engine,
                AskAiArgs {
                    id: args.id,
                    intent: "continue".into(),
                    question: None,
                    paths: None,
                },
                hub,
            )
            .await?;
            Ok(Some(AskPlan {
                prompt: format!(
                    "The developer hands this worktree back to you.\n{}",
                    plan.prompt
                ),
                ..plan
            }))
        }
        _ => Ok(None),
    }
}

pub fn finish_driver(
    engine: &Arc<LocalSync>,
    args: &SetLocalDriverArgs,
    plan: Option<&AskPlan>,
) -> Result<LocalWorkspaceRow, IpcError> {
    if let Some(p) = plan {
        finish_ask(engine, args.id, &p.clear)?;
    }
    lock(&engine.store)?.set_local_workspace_driver(args.id, &args.driver, now_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> LocalWorkspaceRow {
        LocalWorkspaceRow {
            id: 1,
            host_alias: "devbox".into(),
            owner: "acme".into(),
            repo: "app".into(),
            project_id: Some(7),
            worktree_key: "feat".into(),
            remote_path: "/srv/app-feat".into(),
            local_path: "/home/me/fleet/app-feat".into(),
            session_id: None,
            paused: false,
            excludes: vec![],
            state: "synced".into(),
            last_sync_at: None,
            last_error: None,
            pending_local: 0,
            pending_remote: 0,
            skipped: 0,
            conflicts: vec![],
            created_at: 0,
            driver: "shared".into(),
            driver_since: None,
            local_activity: 0,
            remote_activity: 0,
        }
    }

    fn f(p: &str, c: &str) -> (String, String) {
        (p.into(), c.into())
    }

    #[test]
    fn a_prompt_names_the_files_and_asks_for_the_intent() {
        let p = compose(
            &link(),
            "review",
            None,
            &[
                f("src/a.rs", "modified"),
                f("src/new.rs", "added"),
                f("old.txt", "deleted"),
            ],
        )
        .unwrap();
        assert!(p.contains("/home/me/fleet/app-feat"), "{p}");
        assert!(
            p.contains("- M src/a.rs\n- A src/new.rs\n- D old.txt\n"),
            "{p}"
        );
        assert!(p.contains("git diff"), "{p}");
        assert!(p.contains("Review these changes"), "{p}");
    }

    #[test]
    fn a_long_list_is_cut_and_says_how_many_more() {
        let files: Vec<_> = (0..MAX_LISTED + 5)
            .map(|i| f(&format!("f{i}.txt"), "modified"))
            .collect();
        let p = compose(&link(), "continue", None, &files).unwrap();
        assert_eq!(p.matches("\n- M ").count() + 1, MAX_LISTED + 1);
        assert!(p.contains("… and 5 more"), "{p}");
    }

    #[test]
    fn no_files_points_at_git_status_instead() {
        let p = compose(&link(), "explain", None, &[]).unwrap();
        assert!(p.contains("`git status`"), "{p}");
        assert!(!p.contains("- M"), "{p}");
    }

    #[test]
    fn custom_needs_a_question_and_resolve_exactly_one_file() {
        assert_eq!(
            compose(&link(), "custom", Some("  "), &[])
                .unwrap_err()
                .code,
            codes::E_INVALID
        );
        let p = compose(&link(), "custom", Some("Why this?"), &[]).unwrap();
        assert!(p.ends_with("Why this?"), "{p}");
        assert_eq!(
            compose(&link(), "resolve", None, &[]).unwrap_err().code,
            codes::E_INVALID
        );
        let p = compose(&link(), "resolve", None, &[f("src/a.rs", "modified")]).unwrap();
        assert!(p.contains("`src/a.rs.fleet-local`"), "{p}");
        assert_eq!(
            compose(&link(), "dance", None, &[]).unwrap_err().code,
            codes::E_INVALID
        );
    }

    #[test]
    fn any_intent_takes_an_optional_question() {
        let files = [f("a.rs", "modified")];
        let p = compose(&link(), "review", Some(" check the error paths "), &files).unwrap();
        assert!(p.contains("Review these changes"), "{p}");
        assert!(p.ends_with("\nAlso: check the error paths"), "{p}");
        let p = compose(&link(), "merge", Some("keep main's lockfile"), &files).unwrap();
        assert!(p.ends_with("\nAlso: keep main's lockfile"), "{p}");
        let p = compose(&link(), "review", Some("   "), &files).unwrap();
        assert!(!p.contains("Also:"), "{p}");
        let p = compose(&link(), "custom", Some("Why this?"), &files).unwrap();
        assert!(!p.contains("Also:"), "{p}");
    }

    #[test]
    fn every_intent_composes() {
        for intent in ASK_INTENTS {
            let files = [f("a.rs", "modified")];
            let q = (intent == "custom").then_some("q");
            assert!(compose(&link(), intent, q, &files).is_ok(), "{intent}");
        }
    }
}
