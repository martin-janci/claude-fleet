//! Spawning a review session for an existing session.

use super::*;

#[derive(Deserialize)]
pub struct SpawnReviewArgs {
    pub source_session_id: i64,
    pub prompt: String,
    // Reserved for future cancellation wiring. The frontend's
    // invokeCmdAbortable injects a call_id; v1 spawn_review doesn't register a
    // CancellationToken under it (the spawn is short — tmux create + reconcile
    // + ~1.5s seed delay), so an abort is currently a no-op on the backend.
    #[allow(dead_code)]
    pub call_id: Option<u64>,
}

pub async fn spawn_review(
    args: SpawnReviewArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<crate::store::SessionRow, IpcError> {
    // 1. Snapshot source + capture cwd-resolution inputs under a brief lock.
    //    For remote hosts the cwd is finalized off-lock via `ssh.remote_home`.
    let (source, cwd_src) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let source = s
            .get_session_by_id(args.source_session_id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "source session not found"))?;
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
    //    A review runs Claude Code — same pane command as any "work" session.
    let short = format!("{:x}", now_unix() & 0xfffff);
    let review_name = format!("{}--review-{}", source.tmux_name, short);
    let claude_id = uuid::Uuid::new_v4().to_string();
    let tmux = exec_for(&source.host_alias, ssh);
    tmux.new_session(
        &review_name,
        std::path::Path::new(&cwd),
        &crate::tmux::pane_command_for(Some(&claude_id)),
    )
    .await?;

    // 3. Register via per-host reconcile.
    reconcile_one_host(store, ssh, &source.host_alias).await?;

    // 4. Tag as review + capture id.
    let review_id = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let row = s
            .list_sessions_for_host(&source.host_alias)?
            .into_iter()
            .find(|r| r.tmux_name == review_name)
            .ok_or_else(|| IpcError::new("E_INTERNAL", "review session vanished after spawn"))?;
        s.set_session_kind(row.id, "review", Some(source.id))?;
        let _ = s.set_claude_session_id(row.id, &claude_id);
        let _ = s.set_started_at(row.id, now_unix());
        row.id
    };

    // 5. Seed the prompt. Wait until cl's TUI is ready before send-keys lands.
    wait_for_repl_ready(tmux.as_ref(), &review_name).await;
    // Soft-fail: the review session is already spawned, registered, and tagged.
    // If seeding the prompt fails (e.g. cl wasn't ready yet), DON'T discard the
    // session — return it anyway so the user can type the review prompt manually
    // in the terminal. Log the failure for diagnostics.
    if let Err(e) = send_prompt_inner(
        store,
        ssh,
        &source.host_alias,
        &review_name,
        &args.prompt,
        true,
    )
    .await
    {
        tracing::warn!(
            session = %review_name,
            error = %e,
            "[spawn_review] seeding the review prompt failed (the session is live; seed it manually)"
        );
    }

    // 6. Return the tagged review row.
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.get_session_by_id(review_id)?
        .ok_or_else(|| IpcError::new("E_INTERNAL", "review row missing after tag"))
}
