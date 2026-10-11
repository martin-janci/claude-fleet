//! Spawning a review session for an existing session.

use super::*;
use crate::ipc_error::codes;
use crate::ipc_error::lock;

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "SpawnReviewParams")]
pub struct SpawnReviewArgs {
    /// The session to review.
    pub source_session_id: i64,
    /// The review prompt.
    pub prompt: String,
    /// Who reviews: `claude` (the default) or `codex` (M15 G7.12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    // Reserved for future cancellation wiring. The frontend's
    // invokeCmdAbortable injects a call_id; v1 spawn_review doesn't register a
    // CancellationToken under it (the spawn is short — tmux create + reconcile
    // + ~1.5s seed delay), so an abort is currently a no-op on the backend.
    // Skipped on both sides so the MCP schema (which takes this struct
    // directly) never exposes it; a missing `Option` deserialises to `None`.
    #[allow(dead_code)]
    #[serde(skip)]
    #[schemars(skip)]
    pub call_id: Option<u64>,
    /// Who asked for the review (migration 124), set in Rust by the MCP
    /// tool from its connection and never read from a request. `None` = a
    /// person: the review is recorded as the SOURCE owner's start, as its
    /// ownership is.
    #[serde(skip)]
    #[schemars(skip)]
    pub origin: Option<crate::store::SessionOrigin>,
}

/// The reviewer agent a [`SpawnReviewArgs::agent`] names: Claude Code when
/// none, Codex on request. Anything else (a shell, agy, which is not
/// validated yet, an unknown name) is `E_INVALID`.
pub(crate) fn reviewer_agent(
    agent: Option<&str>,
) -> Result<&'static dyn crate::agent_adapter::AgentAdapter, IpcError> {
    match agent.map(str::trim).filter(|a| !a.is_empty()) {
        None | Some(crate::store::AGENT_CLAUDE) => Ok(crate::agent_adapter::claude()),
        Some(crate::store::AGENT_CODEX) => crate::agent_adapter::by_id(crate::store::AGENT_CODEX)
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "no Codex adapter")),
        Some(other) => Err(IpcError::new(
            codes::E_INVALID,
            format!("a review runs claude or codex, not {other}"),
        )),
    }
}

pub async fn spawn_review(
    args: SpawnReviewArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<crate::store::SessionRow, IpcError> {
    let agent = reviewer_agent(args.agent.as_deref())?;
    // 1. Snapshot source + capture cwd-resolution inputs under a brief lock.
    //    For remote hosts the cwd is finalized off-lock via `ssh.remote_home`.
    let (source, cwd_src) = {
        let s = lock(store)?;
        let source = s
            .get_session_by_id(args.source_session_id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "source session not found"))?;
        let cwd_src = cwd_source_for_session(&s, &source)?;
        (source, cwd_src)
    };
    // Automatic workspace check (create-only) in the SOURCE's workspace, and
    // use the directory the probe resolved on the host. The remote guess in
    // `resolve_cwd_source` assumes `.claude/worktrees/`, so on a
    // `.worktrees/`-layout host it named a missing dir and tmux silently fell
    // back to $HOME. Orphans / bg rows keep the plain resolution.
    let cwd = match crate::service::repair::ensure_session_workspace(
        source.id,
        crate::service::repair::Entry::SpawnReview,
        store,
        ssh,
    )
    .await
    {
        Ok(rep) => rep.cwd,
        Err(e) if e.code == codes::E_NOREPO || e.code == codes::E_BG_SESSION => {
            resolve_cwd_source(cwd_src, &source.host_alias, ssh).await?
        }
        Err(e) => return Err(e),
    };

    // 2. Spawn the review tmux session (off-lock).
    //    A review runs its agent's plain pane command, as a "work" session
    //    of that agent would: Claude Code under a minted conversation id,
    //    Codex fresh (it picks its own id).
    let short = format!("{:x}", now_unix() & 0xfffff);
    let review_name = format!("{}--review-{}", source.tmux_name, short);
    let launch = crate::tmux::ClaudeLaunch::default();
    let claude_id = agent.mint_conversation_id();
    let pane = match claude_id.as_deref() {
        Some(id) => agent.launch_command(Some(id), &review_name, &launch),
        None => agent.start_command(&review_name, &launch),
    };
    let tmux = exec_for(&source.host_alias, ssh);
    tmux.new_session(&review_name, std::path::Path::new(&cwd), &pane)
        .await?;

    // The review name is live now; clear any kill of it so step 3 may
    // insert the row (the suffix makes a collision unlikely, not impossible).
    // A review's owner is the SOURCE row's, and step 4 writes it against the
    // row id (multi-user M1, T5) — see there for why the source and not the
    // caller. No path states an owner by NAME any more.
    record_tmux_created(store, &source.host_alias, &review_name);
    // 3. Register via per-host reconcile.
    reconcile_one_host(store, ssh, &source.host_alias).await?;

    // 4. Tag as review + capture id.
    let review_id = {
        let s = lock(store)?;
        let row = s
            .list_sessions_for_host(&source.host_alias)?
            .into_iter()
            .find(|r| r.tmux_name == review_name)
            .ok_or_else(|| {
                IpcError::new(codes::E_INTERNAL, "review session vanished after spawn")
            })?;
        s.set_session_kind(row.id, "review", Some(source.id))?;
        // Not soft, as in `new_session`: a Codex pane on a row that says
        // Claude would be resumed, read and answered as Claude's.
        if agent.id() != crate::store::AGENT_CLAUDE {
            s.set_session_agent(row.id, agent.id())?;
        }
        // Ownership (multi-user M1, T5 / spec §4.3 invariant 6): a review
        // inherits the SOURCE row's owner and visibility, never the caller's.
        // A review runs Claude in the owner's worktree, on the owner's host,
        // over the owner's work — if it belonged to whoever asked for it, a
        // watcher would end up owning a session inside the owner's checkout,
        // with a terminal and outside anything the owner could revoke. (The
        // source row is the authority, so no `Caller` is needed; spawning a
        // review of a session you do not own is refused at the gate, one
        // layer up, because `spawn_review` is an `own` operation.)
        //
        // Hard failure, for `finalize_new_session`'s reason: a review of a
        // private session that lands `unclaimed` is a row the owner cannot
        // read and somebody else could claim. An unowned source leaves the
        // review unclaimed too, which is the right answer, not an error.
        s.claim_if_unclaimed(row.id, source.owner_person_id)?;
        s.set_session_origin(
            row.id,
            &args
                .origin
                .clone()
                .unwrap_or_else(|| crate::store::SessionOrigin::person(source.owner_person_id)),
        )?;
        if let Some(id) = claude_id.as_deref() {
            let _ = s.set_claude_session_id(row.id, id);
        }
        let _ = s.set_started_at(row.id, now_unix());
        row.id
    };

    // 5. Seed the prompt once cl's REPL reads input (`seed`: never into a
    //    dialog, and resubmitted when its Enter is lost). Soft-fail: the
    //    review session is already spawned, registered, and tagged, so an
    //    unseeded one is returned anyway and the user types the prompt.
    let row = {
        let s = lock(store)?;
        s.get_session_by_id(review_id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "review row missing after tag"))?
    };
    if !super::seed::seed_now_as(
        store,
        ssh,
        &row,
        &args.prompt,
        super::prompt::Origin::Unlabeled,
    )
    .await
    {
        tracing::warn!(
            session = %review_name,
            "[spawn_review] the review prompt was not typed (the session is live; seed it manually)"
        );
    }

    // 6. Return the tagged review row.
    let s = lock(store)?;
    s.get_session_by_id(review_id)?
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "review row missing after tag"))
}

#[cfg(test)]
mod reviewer_tests {
    use super::reviewer_agent;

    #[test]
    fn a_review_runs_claude_unless_codex_is_asked_for() {
        assert_eq!(reviewer_agent(None).unwrap().id(), "claude");
        assert_eq!(reviewer_agent(Some("")).unwrap().id(), "claude");
        assert_eq!(reviewer_agent(Some("claude")).unwrap().id(), "claude");
        assert_eq!(reviewer_agent(Some("codex")).unwrap().id(), "codex");
        for bad in ["shell", "agy", "gemini"] {
            let e = reviewer_agent(Some(bad)).err().expect(bad);
            assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
        }
    }
}
