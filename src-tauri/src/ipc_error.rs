use serde::Serialize;
use std::fmt;

/// Canonical `IpcError::code` values. The frontend (`src/lib/*.ts`) and the
/// control-API skill match on these strings, so they are stable identifiers:
/// never rename one without grepping both. New sites should pick a code from
/// here rather than inventing a near-duplicate (`E_DB` / `E_REPO` for a
/// rusqlite failure were consolidated onto `E_SQLITE`).
#[allow(dead_code)]
pub mod codes {
    /// A `std::sync::Mutex` guard (store, MCP runtime, …) was poisoned.
    pub const E_LOCK: &str = "E_LOCK";
    /// A rusqlite call failed (also the `From<rusqlite::Error>` mapping).
    pub const E_SQLITE: &str = "E_SQLITE";
    /// A non-SQLite `std::io::Error` (also the `From<std::io::Error>` mapping).
    pub const E_IO: &str = "E_IO";
    /// Caller-supplied value failed validation (`validate.rs`, arg checks).
    pub const E_INVALID: &str = "E_INVALID";
    /// Semantic validation of a request (e.g. message sender == recipient).
    pub const E_VALIDATE: &str = "E_VALIDATE";
    /// The addressed host / session / project / worktree row does not exist.
    pub const E_NOTFOUND: &str = "E_NOTFOUND";
    /// The target exists but is in the wrong state for the operation.
    pub const E_INVALID_STATE: &str = "E_INVALID_STATE";
    /// An invariant the code relies on was violated (row vanished mid-op…).
    pub const E_INTERNAL: &str = "E_INTERNAL";
    /// The operation was cancelled via the cancellation registry.
    pub const E_CANCELLED: &str = "E_CANCELLED";
    /// A remote/local command exceeded its deadline.
    pub const E_TIMEOUT: &str = "E_TIMEOUT";
    /// Serialising a value (settings JSON…) failed.
    pub const E_SERIALIZE: &str = "E_SERIALIZE";
    /// Parsing external text (settings JSON, command output…) failed.
    pub const E_PARSE: &str = "E_PARSE";
    /// An `ssh` command exceeded its wall-clock bound after connecting; the
    /// child was killed and the host's ControlMaster may have been reset
    /// (`ssh.rs` `run_child`). Distinct from `E_TIMEOUT` so the UI can tell
    /// a wedged transport from a slow local/remote command.
    pub const E_SSH_TIMEOUT: &str = "E_SSH_TIMEOUT";
    /// The `ssh` transport itself failed (ControlMaster, exit status…).
    pub const E_SSH: &str = "E_SSH";
    /// The host is known but currently unreachable.
    pub const E_HOST_OFFLINE: &str = "E_HOST_OFFLINE";
    /// A host probe (version / reachability check) failed.
    pub const E_PROBE: &str = "E_PROBE";
    /// Host provisioning (bootstrap script) failed.
    pub const E_PROVISION: &str = "E_PROVISION";
    /// Uploading a file to a remote host failed.
    pub const E_UPLOAD: &str = "E_UPLOAD";
    /// A `tmux` command failed.
    pub const E_TMUX: &str = "E_TMUX";
    /// The PTY could not be spawned / written / resized.
    pub const E_PTY: &str = "E_PTY";
    /// The PTY was closed under the caller.
    pub const E_PTY_CLOSED: &str = "E_PTY_CLOSED";
    /// Spawning a local process failed.
    pub const E_SPAWN: &str = "E_SPAWN";
    /// A local `bash` helper failed to spawn.
    pub const E_SHELL: &str = "E_SHELL";
    /// A `git` command failed.
    pub const E_GIT: &str = "E_GIT";
    /// Git identity / remote setup on a host failed.
    pub const E_GIT_SETUP: &str = "E_GIT_SETUP";
    /// A repo-browsing command (`commands/repo.rs`) failed.
    pub const E_REPO: &str = "E_REPO";
    /// The session has no project / repo to run a repo operation against.
    pub const E_NOREPO: &str = "E_NOREPO";
    /// The session has no worktree attached.
    pub const E_NO_WORKTREE: &str = "E_NO_WORKTREE";
    /// The worktree is occupied by a live session.
    pub const E_WORKTREE_BUSY: &str = "E_WORKTREE_BUSY";
    /// `git worktree remove` failed.
    pub const E_WORKTREE_REMOVE: &str = "E_WORKTREE_REMOVE";
    /// The worktree has uncommitted / unpushed work.
    pub const E_DIRTY: &str = "E_DIRTY";
    /// The `claude` CLI exited non-zero.
    pub const E_CLAUDE_CLI: &str = "E_CLAUDE_CLI";
    /// The target is a background (`claude --bg`) session with no tmux pane.
    pub const E_BG_SESSION: &str = "E_BG_SESSION";
    /// The session's tmux pane is not alive.
    pub const E_NOT_ALIVE: &str = "E_NOT_ALIVE";
    /// The MCP server is not running.
    pub const E_NOT_RUNNING: &str = "E_NOT_RUNNING";
    /// No MCP bearer token is configured.
    pub const E_NO_TOKEN: &str = "E_NO_TOKEN";
    /// A session tried to target itself (send_message, spawn_review…).
    pub const E_SELF_TARGET: &str = "E_SELF_TARGET";
    /// A safe-kill is already in progress for the session.
    pub const E_SAFE_KILL_IN_PROGRESS: &str = "E_SAFE_KILL_IN_PROGRESS";
    /// Clipboard read/write failed.
    pub const E_CLIPBOARD: &str = "E_CLIPBOARD";
    /// No clipboard backend is available on this host.
    pub const E_CLIPBOARD_UNAVAILABLE: &str = "E_CLIPBOARD_UNAVAILABLE";
    /// The home directory could not be resolved.
    pub const E_HOME: &str = "E_HOME";
    /// The operation is not supported on this platform / session kind.
    pub const E_UNSUPPORTED: &str = "E_UNSUPPORTED";
    /// The caller's identity/mode does not permit the operation (readonly
    /// token on a mutating tool, per-host token acting on another host or
    /// on a fleet-admin tool, a denied confirmation, an upload path that
    /// was never dropped onto the window).
    pub const E_FORBIDDEN: &str = "E_FORBIDDEN";
    /// The caller exceeded a per-caller rate limit (`broadcast_prompt`);
    /// `details.retry_after_secs` says when to retry.
    pub const E_RATE_LIMITED: &str = "E_RATE_LIMITED";
    /// The call needs a desktop confirmation first (`mcp.confirm_destructive`);
    /// retry with the `confirm_nonce` from `details` once approved.
    pub const E_CONFIRM_REQUIRED: &str = "E_CONFIRM_REQUIRED";
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct IpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[allow(dead_code)]
impl IpcError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// The error every `store.lock().map_err(..)` site returns when the
    /// `Store` (or another app-level) mutex is poisoned.
    pub fn lock() -> Self {
        Self::new(codes::E_LOCK, "store mutex poisoned")
    }
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for IpcError {}

impl From<rusqlite::Error> for IpcError {
    fn from(e: rusqlite::Error) -> Self {
        Self::new(codes::E_SQLITE, e.to_string())
    }
}

impl From<std::io::Error> for IpcError {
    fn from(e: std::io::Error) -> Self {
        Self::new(codes::E_IO, e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_without_details() {
        let err = IpcError::new("E_TEST", "boom");
        let s = serde_json::to_string(&err).unwrap();
        assert_eq!(s, r#"{"code":"E_TEST","message":"boom"}"#);
    }

    #[test]
    fn serializes_with_details() {
        let err = IpcError::new("E_TEST", "boom").with_details(serde_json::json!({ "path": "/x" }));
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains(r#""code":"E_TEST""#));
        assert!(s.contains(r#""message":"boom""#));
        assert!(s.contains(r#""details":{"path":"/x"}"#));
    }

    #[test]
    fn lock_constructor_uses_e_lock_code() {
        let err = IpcError::lock();
        assert_eq!(err.code, codes::E_LOCK);
        assert_eq!(err.message, "store mutex poisoned");
    }

    #[test]
    fn from_rusqlite_error_uses_e_sqlite_code() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let sql_err = conn.execute("SELECT * FROM no_such_table", []).unwrap_err();
        let err: IpcError = sql_err.into();
        assert_eq!(err.code, codes::E_SQLITE);
        assert!(!err.message.is_empty());
    }
}
