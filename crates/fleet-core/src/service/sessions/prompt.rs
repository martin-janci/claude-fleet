//! Sending prompts to sessions (single and broadcast), recording the prompt
//! outcome and timeline events, and capturing pane output.

use super::*;
use crate::ipc_error::codes;
use crate::ipc_error::lock;

/// Largest prompt body (bytes, after normalisation) a single send carries.
/// The body rides the command line base64-encoded (ssh has no stdin path on
/// an agent-routed host): 64 KiB × 4/3 stays under Linux's 128 KiB per-argv
/// string, with room for the script around it.
pub const MAX_PROMPT_BYTES: usize = 65_536;

/// Make a prompt body safe to type: `\r\n` and a lone `\r` become `\n` (a
/// CRLF client's prompt otherwise reaches the REPL as several submissions,
/// since `\r` is Return there); any other control character except `\n` and
/// `\t` is refused — an ESC or Ctrl-C byte in a body would interrupt or kill
/// the recipient. Caps the size at [`MAX_PROMPT_BYTES`].
pub fn normalize_prompt_body(prompt: &str) -> Result<String, IpcError> {
    let folded = prompt.replace("\r\n", "\n").replace('\r', "\n");
    if let Some(c) = folded
        .chars()
        .find(|c| c.is_control() && *c != '\n' && *c != '\t')
    {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!(
                "prompt contains a control character (U+{:04X}); only newline and tab are allowed",
                c as u32
            ),
        ));
    }
    if folded.len() > MAX_PROMPT_BYTES {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!(
                "prompt is {} bytes; the limit is {MAX_PROMPT_BYTES} bytes",
                folded.len()
            ),
        ));
    }
    Ok(folded)
}

/// The one shell script that delivers a prompt to a session, in a single
/// round trip:
///
/// 1. pick the target: the row's known pane id when it still belongs to
///    this session (a split window's active pane is the shell, not Claude),
///    else the EXACT session target `=<name>:` (a bare name would let tmux
///    prefix-match another session);
/// 2. `printf … | base64 -d | tmux load-buffer -b <buffer> -` — the body
///    never touches shell quoting, tmux key-name parsing or the 150 ms
///    typing race;
/// 3. `tmux paste-buffer -p -d` — bracketed paste when the pane asked for
///    it (Claude Code does), so the REPL sees one paste with an unambiguous
///    end marker and internal newlines stay soft; on failure (e.g. a stale
///    target) the buffer is explicitly deleted so it can't leak on the
///    host, and the chain stops there — Enter never fires after a failed
///    paste;
/// 4. when `submit`, a short settle and ONE Enter.
///
/// A single trailing newline is stripped so it cannot pre-submit the body.
/// An empty body presses Enter only when `submit` is true (the Conversation
/// tab's "Press Enter" chip); with `submit = false` an empty body is a
/// no-op — nothing is typed and nothing is pressed.
pub fn build_send_script(
    tmux_name: &str,
    pane_id: Option<&str>,
    body: &str,
    buffer: &str,
    submit: bool,
) -> String {
    use base64::Engine as _;
    let body = body.strip_suffix('\n').unwrap_or(body);
    let exact = quote(&crate::tmux::exact_pane(tmux_name));
    let mut script = format!("t={exact}; ");
    if let Some(pane) = pane_id {
        let pane_q = quote(pane);
        let name_q = quote(tmux_name);
        script.push_str(&format!(
            "if [ \"$(tmux display-message -p -t {pane_q} '#{{session_name}}' 2>/dev/null)\" = {name_q} ]; then t={pane_q}; fi; "
        ));
    }
    if body.is_empty() {
        if submit {
            script.push_str("tmux send-keys -t \"$t\" Enter");
        } else {
            // Nothing to type and nothing to press: a no-op that still
            // leaves the target-selection prefix as a syntactically valid
            // script.
            script.push(':');
        }
        return script;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
    let buf_q = quote(buffer);
    script.push_str(&format!(
        "printf %s {} | base64 -d | tmux load-buffer -b {buf_q} - && tmux paste-buffer -p -d -b {buf_q} -t \"$t\" || {{ tmux delete-buffer -b {buf_q}; false; }}",
        quote(&b64)
    ));
    if submit {
        script.push_str(" && sleep 0.15 && tmux send-keys -t \"$t\" Enter");
    }
    script
}

pub(super) fn default_submit() -> bool {
    true
}

/// PURE: whether a sent body counts as a prompt worth recording (anything
/// but whitespace).
pub fn is_prompt(body: &str) -> bool {
    !body.trim().is_empty()
}

#[derive(Serialize, Deserialize)]
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
    let body = normalize_prompt_body(prompt)?;
    // The pane Claude runs in, when reconcile or a hook has told us. Lock,
    // read, unlock — never across the send.
    let pane_id = {
        let s = lock(store)?;
        s.get_session(tmux_name, host_alias)?
            .and_then(|r| r.context.tmux_pane_id)
    };
    let buffer = format!("fleet-{}", uuid::Uuid::new_v4().simple());
    let script = build_send_script(tmux_name, pane_id.as_deref(), &body, &buffer, submit);
    let out = if host_alias == "local" {
        crate::service::hub::ensure_local_allowed(host_alias)?;
        tokio::process::Command::new("bash")
            .args(["-c", &script])
            .output()
            .await
            .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn bash: {e}")))?
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
            codes::E_TMUX,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    // Task G: record the prompt on the session's timeline (detail truncated to
    // ~120 chars). Append-only + best-effort: never fail the send on this.
    // A bare Enter (empty body: the Conversation tab's "Press Enter" chip
    // for a stuck session) is a key press, not a prompt: nothing to record,
    // and it must not blank the row's last_prompt.
    if is_prompt(&body) {
        record_session_event(store, host_alias, tmux_name, "prompt_sent", {
            // What was DELIVERED keeps the untrusted marker; what fleet records
            // does not (D8 / Q2). The marker line alone is ~77 chars, so without
            // this the 120-char detail is almost entirely marker.
            let truncated: String = crate::mcp::guard::strip_marker(&body)
                .chars()
                .take(120)
                .collect();
            Some(truncated)
        });
        record_prompt_outcome(store, host_alias, tmux_name, &body);
    }
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
    // An MCP-delivered prompt arrives with the untrusted marker as its first
    // line. The session was shown it; `last_prompt` and the derived label must
    // not be it (D8 / Q2). Stripped once here, so both the Tauri and the MCP
    // path are covered and `mcp/tools.rs` needs no change.
    let prompt = crate::mcp::guard::strip_marker(prompt);
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
///   - the operator session `(host_alias, tmux_name)`, when recorded, is
///     excluded so a fan-out never prompts the UX agent that may have sent it.
pub fn select_targets(
    sessions: &[SessionRow],
    f: &BroadcastFilter,
    controller: Option<&(String, String)>,
    operator: Option<&(String, String)>,
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
        .filter(|s| match operator {
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
    // Snapshot sessions + resolve the controller and the operator while
    // holding the guard, then drop it before any `.await` (never hold the
    // mutex across await).
    let (sessions, controller, operator) = {
        let s = lock(store)?;
        let sessions = s.list_all_sessions().map_err(|e| {
            IpcError::new(codes::E_SQLITE, format!("list sessions for broadcast: {e}"))
        })?;
        // The controller concept is resolved from the store when available.
        // Until a controller is recorded, no session is excluded on that basis.
        let controller = resolve_controller(&s);
        // Likewise the operator: absent a recorded UX-agent session, nothing
        // is excluded on that basis.
        let operator =
            crate::service::operator::operator_ref(&s).map(|r| (r.host_alias, r.tmux_name));
        (sessions, controller, operator)
    };

    let targets = select_targets(&sessions, &filter, controller.as_ref(), operator.as_ref());

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
    let (host, name) = crate::service::repo::session_target(store, session_id)?;
    let tmux = exec_for(&host, ssh);
    match scrollback_lines {
        Some(n) => tmux.capture_pane_scrollback(&name, n).await,
        None => tmux.capture_pane(&name).await,
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    #[test]
    fn a_bare_enter_is_not_a_prompt() {
        assert!(!is_prompt(""));
        assert!(!is_prompt("  \n"));
        assert!(is_prompt("/clear"));
        assert!(is_prompt("fix it"));
    }
}
