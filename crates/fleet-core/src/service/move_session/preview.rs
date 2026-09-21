//! A read-only "preview" of a move: the move's own opening checks
//! (`gather`), plus the read-only target probes (`probe`), plus the source
//! listings the move itself would run before deciding what to carry. Nothing
//! here writes to either host — see `probe`'s module docs for the discipline
//! that keeps a single git call from becoming a write, and
//! `docs/superpowers/specs/2026-09-21-transfer-preflight` (slice 3) for why
//! this exists.
//!
//! A refusal from [`preview`] is `gather()`'s own `Err`, or (for step 0) the
//! same alias validation the move itself runs before taking its claim — the
//! same value the real move would return, not a description of it. Every
//! other question a dry run cannot answer (a probe that failed, a listing
//! that came back empty because nothing answered it, the bundle size that
//! only snapshotting could tell) is reported in `unknowns` instead: the move
//! never refuses on any of those, so a preview must not either.

use super::carry::{self, IgnoredEntry, LeftBehind};
use super::claude_state;
use super::probe::{self, Probed};
use super::{gather, sh, sh_soft, stderr_of, target_paths, MoveHooks, MoveSessionArgs};
use crate::ipc_error::IpcError;
use crate::service::safe_kill::DirtyFile;
use crate::ssh::SshExec;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;

/// ssh connect timeout for the read-only probes: local git only, same bound
/// as the move's own git inspection.
const PROBE_TIMEOUT: Duration = Duration::from_secs(40);
/// Timeout for the source listings (ignored files, session state, memory):
/// the same bound the move itself uses for these.
const LISTING_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovePreview {
    pub session_id: i64,
    pub from_host: String,
    pub to_host: String,
    pub branch: String,
    pub source_cwd: String,
    /// Commits the source has that origin lacks (`SourceState.ahead`); `None`
    /// when git could not say. Always available — unlike `commits_ahead`.
    pub unpushed_commits: Option<u32>,
    /// Commits the TARGET's clone lacks. `None` when it has no clone yet, or
    /// when its branch tip is a commit the source has never seen — then any
    /// number would be a guess. Not zero, which would read as "up to date".
    pub commits_ahead: Option<u32>,
    /// Exactly the rows the carry would replay.
    pub dirty: Vec<DirtyFile>,
    /// `carry::select_ignored`'s split under the move's own caps.
    pub ignored_carried: Vec<IgnoredEntry>,
    pub ignored_left_behind: Vec<LeftBehind>,
    pub transcript_bytes: u64,
    /// Present on the source. The move applies its own size caps to these, so
    /// a very large session directory may not all travel — the result view
    /// says what actually did.
    pub session_state_files: u32,
    pub session_state_bytes: u64,
    pub memory_files: u32,
    pub memory_bytes: u64,
    /// The path the move would aim at (correction 3).
    pub target_path: String,
    pub target: TargetState,
    /// What this preview cannot tell you, in words a person can read.
    pub unknowns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TargetState {
    Absent,
    /// The probe could not answer (an SSH failure, an unreadable reply). The
    /// reason is in `unknowns`. Never a refusal: the move does not probe.
    Unknown,
    Clean {
        head: String,
    },
    /// Deliberately not classified as 3d's `ours` / `theirs`: that verdict
    /// needs `refs/fleet/transfer/<id>/*`, which a dry run never creates.
    Dirty {
        head: String,
        entries: Vec<DirtyFile>,
    },
}

/// Run `script` on `host` and return its stdout bytes on a clean exit, or a
/// human-readable reason otherwise — a transport failure and a non-zero exit
/// read the same to a caller that only ever warns.
async fn probe_bytes(
    ssh: &dyn SshExec,
    host: &str,
    script: &str,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    match sh(ssh, host, script, timeout).await {
        Ok(out) if out.status.success() => Ok(out.stdout),
        Ok(out) => Err(stderr_of(&out)),
        Err(e) => Err(e.message),
    }
}

/// [`probe_bytes`], decoded lossily — for the probes whose parsers work on
/// `&str` (they only ever carry short, ASCII-safe git answers, never a raw
/// file listing).
async fn probe_text(
    ssh: &dyn SshExec,
    host: &str,
    script: &str,
    timeout: Duration,
) -> Result<String, String> {
    probe_bytes(ssh, host, script, timeout)
        .await
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// A read-only run of the move's own opening checks, plus the probes the move
/// does not need. A refusal is `Err` with the code and message the real move
/// would return — the same value, not a description of it.
// Not called outside its own tests yet: Task 4 of the transfer-preflight
// slice wires this into a command / MCP tool that returns `MovePreview` to
// the UI.
#[allow(dead_code)]
pub(super) async fn preview(
    args: &MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
) -> Result<MovePreview, IpcError> {
    // 0. The move validates the alias *before* taking its claim, which is
    // outside `gather()` — a dry run must do it itself, or it would accept an
    // alias the move refuses.
    crate::validate::host_alias(&args.target_host_alias)?;

    // 1. The move's own opening checks, verbatim. A refusal here is the
    // move's own error.
    let g = gather(args, store, ssh, hooks, None).await?;

    let mut unknowns: Vec<String> = Vec::new();

    // 2. The small git-ignored files: never a refusal — the move only warns
    // on this half too.
    let (ignored_carried, ignored_left_behind) = match probe_bytes(
        ssh,
        &g.src,
        &carry::ignored_list_script(&g.state.worktree),
        LISTING_TIMEOUT,
    )
    .await
    {
        Ok(stdout) => {
            let listed = carry::parse_ignored_list(&stdout);
            let sel =
                carry::select_ignored(listed, g.snap.ignored_entry_kb, g.snap.ignored_total_kb);
            (sel.carry, sel.left)
        }
        Err(why) => {
            unknowns.push(format!(
                "the git-ignored files on {} could not be listed: {why}",
                g.src
            ));
            (Vec::new(), Vec::new())
        }
    };

    // 3. The per-session directory and the project's memory, as counts and
    // byte totals — both warn-only, exactly as the move treats them.
    let (session_state_files, session_state_bytes) =
        match std::path::Path::new(&g.located.path).parent() {
            Some(dir) => {
                let src_project_dir = dir.to_string_lossy().into_owned();
                match sh_soft(
                    ssh,
                    &g.src,
                    &claude_state::session_list_script(&src_project_dir, &g.id),
                    "listing the session directory",
                )
                .await
                {
                    Ok(out) if carry::payload(&out.stdout).is_none() => {
                        unknowns.push(format!(
                            "the session-state listing on {} said nothing",
                            g.src
                        ));
                        (0, 0)
                    }
                    Ok(out) => {
                        let listed = claude_state::parse_file_list(&out.stdout);
                        let bytes = listed.iter().map(|f| f.bytes).sum();
                        (listed.len() as u32, bytes)
                    }
                    Err(why) => {
                        unknowns.push(format!("session state was not listed: {why}"));
                        (0, 0)
                    }
                }
            }
            None => {
                unknowns.push(
                    "session state was not listed: the transcript has no parent directory".into(),
                );
                (0, 0)
            }
        };

    let (memory_files, memory_bytes) = match sh_soft(
        ssh,
        &g.src,
        &claude_state::memory_list_script(&g.state.worktree, Some(&g.state.worktree)),
        "listing the memory",
    )
    .await
    {
        Ok(out) => match claude_state::parse_memory_list(&out.stdout) {
            Some(listing) => {
                let bytes = listing.files.iter().map(|f| f.bytes).sum();
                (listing.files.len() as u32, bytes)
            }
            None => {
                unknowns.push(format!("the memory listing on {} was unreadable", g.src));
                (0, 0)
            }
        },
        Err(why) => {
            unknowns.push(format!("the memory was not listed: {why}"));
            (0, 0)
        }
    };

    // 4. Where the move would put the target's clone and worktree. A
    // failure here is the move's own error (it fails on exactly this step).
    let (project_root, cwd_hint) = target_paths(&g.snap, &g.target, ssh).await?;

    // 5. What sits at the path the move would aim at. Never a refusal: the
    // move does not probe, so a preview must not refuse on something the
    // move would never have looked at.
    let target_state = match probe_text(
        ssh,
        &g.target,
        &probe::target_probe_script(&cwd_hint),
        PROBE_TIMEOUT,
    )
    .await
    {
        Ok(stdout) => match probe::parse_target_probe(&stdout) {
            Ok(Probed::Absent) => TargetState::Absent,
            Ok(Probed::Worktree { head, porcelain }) => {
                if porcelain.is_empty() {
                    TargetState::Clean { head }
                } else {
                    TargetState::Dirty {
                        head,
                        entries: porcelain,
                    }
                }
            }
            Err(e) => {
                unknowns.push(format!(
                    "the target's state on {} could not be read: {}",
                    g.target, e.message
                ));
                TargetState::Unknown
            }
        },
        Err(why) => {
            unknowns.push(format!(
                "the target's state on {} could not be read: {why}",
                g.target
            ));
            TargetState::Unknown
        }
    };
    if matches!(target_state, TargetState::Absent) {
        unknowns.push(
            "the target does not exist yet; its state will only exist once the move creates it"
                .into(),
        );
    }

    // 6. The target's branch tip, and (only when one exists) how far the
    // source is ahead of it. Either lookup failing leaves `commits_ahead`
    // `None` with a line in `unknowns` — a tip the target simply does not
    // have yet is not a failure, so it says nothing here.
    let mut commits_ahead = None;
    match probe_text(
        ssh,
        &g.target,
        &probe::target_tip_script(&project_root, &g.snap.branch),
        PROBE_TIMEOUT,
    )
    .await
    {
        Ok(stdout) => match probe::parse_target_tip(&stdout) {
            Ok(Some(tip)) => {
                match probe_text(
                    ssh,
                    &g.src,
                    &probe::commits_ahead_script(&g.state.worktree, &tip),
                    PROBE_TIMEOUT,
                )
                .await
                {
                    Ok(stdout) => match probe::parse_commits_ahead(&stdout) {
                        Ok(n) => commits_ahead = n,
                        Err(e) => unknowns.push(format!(
                            "commits ahead of the target could not be counted: {}",
                            e.message
                        )),
                    },
                    Err(why) => unknowns.push(format!(
                        "commits ahead of the target could not be counted: {why}"
                    )),
                }
            }
            Ok(None) => {}
            Err(e) => unknowns.push(format!(
                "the target's branch tip could not be read: {}",
                e.message
            )),
        },
        Err(why) => unknowns.push(format!("the target's branch tip could not be read: {why}")),
    }

    // 7. What a dry run cannot know, or chooses not to act on.
    unknowns.push(
        "the bundle size is decided by snapshotting the source worktree, which a dry run does \
         not do, so it cannot be known here; it is what E_MOVE_TOO_LARGE depends on"
            .to_string(),
    );
    if args.strict {
        unknowns.push("strict is ignored by a dry run".to_string());
    }
    if args.clean_target {
        unknowns.push("clean_target is ignored by a dry run".to_string());
    }

    Ok(MovePreview {
        session_id: args.session_id,
        from_host: g.src.clone(),
        to_host: g.target.clone(),
        branch: g.snap.branch.clone(),
        source_cwd: g.state.worktree.clone(),
        unpushed_commits: u32::try_from(g.state.ahead).ok(),
        commits_ahead,
        dirty: g.state.dirty.clone(),
        ignored_carried,
        ignored_left_behind,
        transcript_bytes: g.located.size,
        session_state_files,
        session_state_bytes,
        memory_files,
        memory_bytes,
        target_path: cwd_hint,
        target: target_state,
        unknowns,
    })
}
