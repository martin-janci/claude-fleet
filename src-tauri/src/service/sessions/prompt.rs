//! Sending prompts to sessions (single and broadcast), recording the prompt
//! outcome and timeline events, and capturing pane output.

use super::*;

/// Build the tmux invocations that together send a prompt to a session:
///   1. send-keys -t <name> -l <body>   (literal, no key-name translation;
///      a single trailing newline is stripped so internal newlines stay as
///      soft newlines and a stray trailing one can't pre-submit the body)
///   2. (when `submit`) a short settle so the REPL flushes the literal paste
///   3. (when `submit`) send-keys -t <name> Enter   (one real Enter to submit)
///
/// With `submit = false` the body is staged in the REPL but not submitted.
pub fn build_send_commands(tmux_name: &str, prompt: &str, submit: bool) -> Vec<String> {
    let body = prompt.strip_suffix('\n').unwrap_or(prompt);
    let mut cmds = vec![format!(
        "tmux send-keys -t {} -l {}",
        quote(tmux_name),
        quote(body)
    )];
    if submit {
        // settle so the REPL flushes the literal paste before the submit key
        cmds.push("sleep 0.15".to_string());
        cmds.push(format!("tmux send-keys -t {} Enter", quote(tmux_name)));
    }
    cmds
}

pub(super) fn default_submit() -> bool {
    true
}

#[derive(Deserialize)]
pub struct SendPromptArgs {
    pub host_alias: String,
    pub tmux_name: String,
    pub prompt: String,
    #[serde(default = "default_submit")]
    pub submit: bool,
}

pub(super) async fn send_prompt_inner(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    host_alias: &str,
    tmux_name: &str,
    prompt: &str,
    submit: bool,
) -> Result<(), IpcError> {
    crate::validate::host_alias(host_alias)?;
    crate::validate::tmux_name_addressable(tmux_name)?;
    // The send-keys commands run in ONE shell invocation joined with `&&` (so a
    // failed literal-text send doesn't still fire Enter) — one round-trip
    // instead of two.
    let script = build_send_commands(tmux_name, prompt, submit).join(" && ");
    let out = if host_alias == "local" {
        tokio::process::Command::new("bash")
            .args(["-c", &script])
            .output()
            .await
            .map_err(|e| IpcError::new("E_TMUX", format!("spawn bash: {e}")))?
    } else {
        ssh.run(
            host_alias,
            &["bash", "-lc", &quote(&script)],
            std::time::Duration::from_secs(10),
        )
        .await?
    };
    if !out.status.success() {
        return Err(IpcError::new(
            "E_TMUX",
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    // Task G: record the prompt on the session's timeline (detail truncated to
    // ~120 chars). Append-only + best-effort: never fail the send on this.
    record_session_event(store, host_alias, tmux_name, "prompt_sent", {
        let truncated: String = prompt.chars().take(120).collect();
        Some(truncated)
    });
    record_prompt_outcome(store, host_alias, tmux_name, prompt);
    Ok(())
}

/// Derive a default sidebar label from a prompt (PROD-4): the first five
/// words, lowercased, punctuation stripped, capped to the friendly-name
/// limit. `None` when nothing printable is left.
pub fn friendly_name_from_prompt(prompt: &str) -> Option<String> {
    let words: Vec<String> = prompt
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .take(5)
        .collect();
    if words.is_empty() {
        return None;
    }
    let joined = words.join(" ");
    Some(joined.chars().take(80).collect())
}

/// Post-send bookkeeping (PROD-4 / PROD-5): stamp `last_prompt`, and give a
/// still-unnamed session a default friendly name derived from the prompt.
/// Best-effort: every failure is logged and swallowed — the prompt already
/// landed in the pane.
pub(super) fn record_prompt_outcome(
    store: &Mutex<Store>,
    host_alias: &str,
    tmux_name: &str,
    prompt: &str,
) {
    let Ok(s) = store.lock() else {
        tracing::error!(
            host = %host_alias,
            session = %tmux_name,
            "[prompt] store mutex poisoned recording the outcome"
        );
        return;
    };
    let row = match s.get_session(tmux_name, host_alias) {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(
                host = %host_alias,
                session = %tmux_name,
                error = %e,
                "[prompt] lookup failed"
            );
            return;
        }
    };
    if let Err(e) = s.set_last_prompt(row.id, prompt) {
        // Never the prompt text itself: identifiers only.
        tracing::warn!(
            host = %host_alias,
            session = %tmux_name,
            error = %e,
            "[prompt] set_last_prompt failed"
        );
    }
    // The prompt-derived label replaces NO name or the deterministic
    // branch-derived default every fleet-created session starts with; a
    // label a human or the in-session agent chose (set_friendly_name) stays.
    let replaceable = match &row.friendly_name {
        None => true,
        Some(current) => {
            s.default_friendly_name(row.id).ok().flatten().as_deref() == Some(current.as_str())
        }
    };
    if replaceable {
        if let Some(name) = friendly_name_from_prompt(prompt) {
            if let Err(e) = s.set_friendly_name(host_alias, tmux_name, Some(&name)) {
                tracing::warn!(
                    host = %host_alias,
                    session = %tmux_name,
                    error = %e,
                    "[prompt] setting the default friendly_name failed"
                );
            }
        }
    }
}

/// Append one event to a session's timeline, resolving the row by
/// (tmux_name, host). Best-effort: every failure (lock poisoned, row missing,
/// SQL error) is logged and swallowed so it can never block the mutation that
/// produced the event. Shared by send_prompt / kill / recreate. (Task G;
/// reconcile uses an inlined variant because it already holds the lock.)
pub(super) fn record_session_event(
    store: &Mutex<Store>,
    host_alias: &str,
    tmux_name: &str,
    kind: &str,
    detail: Option<String>,
) {
    let s = match store.lock() {
        Ok(s) => s,
        Err(_) => {
            tracing::error!(
                kind,
                host = %host_alias,
                session = %tmux_name,
                "[event] store mutex poisoned recording an event"
            );
            return;
        }
    };
    match s.get_session(tmux_name, host_alias) {
        Ok(Some(row)) => {
            if let Err(e) = s.insert_session_event(row.id, kind, detail.as_deref()) {
                tracing::warn!(
                    kind,
                    host = %host_alias,
                    session = %tmux_name,
                    error = %e,
                    "[event] insert failed"
                );
            }
        }
        Ok(None) => {} // no row yet (e.g. brand-new session) — nothing to attach to
        Err(e) => tracing::warn!(
            kind,
            host = %host_alias,
            session = %tmux_name,
            error = %e,
            "[event] lookup failed"
        ),
    }
}

pub async fn send_prompt(
    args: SendPromptArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    send_prompt_inner(
        store,
        ssh,
        &args.host_alias,
        &args.tmux_name,
        &args.prompt,
        args.submit,
    )
    .await
}

// --- broadcast_prompt (fan-out to matching work sessions) ------------------

/// Filter narrowing which work sessions a broadcast targets. Any field left
/// `None` is not constrained. `status` compares against a session's
/// `claude_status`.
#[derive(Debug, Default, Clone)]
pub struct BroadcastFilter {
    pub host: Option<String>,
    pub project_id: Option<i64>,
    pub status: Option<String>,
}

/// PURE selector: pick the session ids a broadcast should target.
///
/// Rules:
///   - only `kind == "work"` sessions are eligible;
///   - the host/project_id/status filters are applied only when set
///     (status compares against `claude_status`);
///   - the controller `(host_alias, tmux_name)`, when known, is excluded so a
///     broadcast never fans back into the session driving it.
pub fn select_targets(
    sessions: &[SessionRow],
    f: &BroadcastFilter,
    controller: Option<&(String, String)>,
) -> Vec<i64> {
    sessions
        .iter()
        .filter(|s| s.kind == "work")
        .filter(|s| match &f.host {
            Some(h) => &s.host_alias == h,
            None => true,
        })
        .filter(|s| match f.project_id {
            Some(pid) => s.project_id == Some(pid),
            None => true,
        })
        .filter(|s| match &f.status {
            Some(st) => s.claude_status.as_deref() == Some(st.as_str()),
            None => true,
        })
        .filter(|s| match controller {
            Some((host, tmux)) => !(&s.host_alias == host && &s.tmux_name == tmux),
            None => true,
        })
        .map(|s| s.id)
        .collect()
}

/// Per-session outcome of a broadcast.
#[derive(serde::Serialize)]
pub struct BroadcastResult {
    pub session_id: i64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Serializable summary returned by [`broadcast_prompt`].
#[derive(serde::Serialize)]
pub struct BroadcastSummary {
    pub sent: u32,
    pub failed: u32,
    pub results: Vec<BroadcastResult>,
}

/// Fan the same `prompt` out to every work session matching `filter`,
/// excluding the controller. Resolves targets via [`select_targets`] (reading
/// the controller from the store), then delivers via the existing
/// [`send_prompt`] per target, collecting one result each.
///
/// `submit` mirrors `send_prompt`'s submit semantics (Enter after the literal
/// text). It is threaded through for API parity; the current delivery path
/// always submits, so today it is accepted and ignored when `true` (the
/// default). It is kept in the signature so a future no-submit `send_prompt`
/// can wire straight through without a signature change.
pub async fn broadcast_prompt(
    filter: BroadcastFilter,
    prompt: String,
    submit: bool,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<BroadcastSummary, IpcError> {
    // Snapshot sessions + resolve the controller while holding the guard, then
    // drop it before any `.await` (never hold the mutex across await).
    let (sessions, controller) = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let sessions = s.list_all_sessions().map_err(|e| {
            IpcError::new(codes::E_SQLITE, format!("list sessions for broadcast: {e}"))
        })?;
        // The controller concept is resolved from the store when available.
        // Until a controller is recorded, no session is excluded on that basis.
        let controller = resolve_controller(&s);
        (sessions, controller)
    };

    let targets = select_targets(&sessions, &filter, controller.as_ref());

    // Map session id -> (host_alias, tmux_name) for delivery.
    let mut results: Vec<BroadcastResult> = Vec::with_capacity(targets.len());
    let mut sent: u32 = 0;
    let mut failed: u32 = 0;
    for sid in targets {
        let Some(row) = sessions.iter().find(|s| s.id == sid) else {
            continue;
        };
        let res =
            send_prompt_inner(store, ssh, &row.host_alias, &row.tmux_name, &prompt, submit).await;
        match res {
            Ok(()) => {
                sent += 1;
                results.push(BroadcastResult {
                    session_id: sid,
                    ok: true,
                    error: None,
                });
            }
            Err(e) => {
                failed += 1;
                results.push(BroadcastResult {
                    session_id: sid,
                    ok: false,
                    error: Some(format!("{}: {}", e.code, e.message)),
                });
            }
        }
    }

    Ok(BroadcastSummary {
        sent,
        failed,
        results,
    })
}

/// Best-effort controller lookup. The recorded controller `(host_alias,
/// tmux_name)` lives in the store (Task D); broadcast excludes it so a fan-out
/// never steers the controller session into itself. Degrades to "no controller
/// known" (no exclusion) on any store error.
pub(super) fn resolve_controller(store: &Store) -> Option<(String, String)> {
    store.get_controller().ok().flatten()
}

/// Capture a session's terminal output. `scrollback_lines = None` returns the
/// visible pane; `Some(n)` includes `n` rows of scrollback history.
pub async fn capture_session_output(
    session_id: i64,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    scrollback_lines: Option<u32>,
) -> Result<String, IpcError> {
    let (host, name) = crate::commands::repo::session_target(store, session_id)?;
    let tmux = exec_for(&host, ssh);
    match scrollback_lines {
        Some(n) => tmux.capture_pane_scrollback(&name, n).await,
        None => tmux.capture_pane(&name).await,
    }
}
