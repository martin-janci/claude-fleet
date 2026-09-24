use serde::Serialize;
use std::fmt;

/// Canonical `IpcError::code` values. The frontend (`src/lib/*.ts`) and the
/// control-API skill match on these strings, so they are stable identifiers:
/// never rename one without grepping both. New sites should pick a code from
/// here rather than inventing a near-duplicate (`E_DB` / `E_REPO` for a
/// rusqlite failure were consolidated onto `E_SQLITE`).
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
    /// A name-only session lookup matched rows on several hosts; the
    /// candidates ride along in `details` so the caller can pick a host.
    pub const E_AMBIGUOUS: &str = "E_AMBIGUOUS";
    /// The thing being created already exists (a project's owner/repo, a
    /// checkout already at the destination path…) — the inverse of
    /// `E_NOTFOUND`.
    pub const E_EXISTS: &str = "E_EXISTS";
    /// The `gh` CLI failed: not installed, unauthenticated, or exited
    /// non-zero, or its JSON output could not be parsed.
    pub const E_GH: &str = "E_GH";
    /// The target exists but is in the wrong state for the operation.
    pub const E_INVALID_STATE: &str = "E_INVALID_STATE";
    /// The task is already in a terminal state (`done` / `failed` /
    /// `cancelled`), so the requested transition is refused.
    pub const E_TASK_TERMINAL: &str = "E_TASK_TERMINAL";
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
    /// The host uses the `agent` transport and no `fleet-agent` is connected
    /// for it. Returned *immediately*, never after a timeout, so an agent
    /// host reports unreachable as fast as a down SSH host does.
    pub const E_AGENT_OFFLINE: &str = "E_AGENT_OFFLINE";
    /// A connected agent answered with something the protocol does not allow:
    /// a frame that does not answer the request, or a body that will not
    /// decode. Distinct from `E_AGENT_OFFLINE` — the connection is up.
    pub const E_AGENT_PROTOCOL: &str = "E_AGENT_PROTOCOL";
    /// An agent host's token was just minted or rotated. It is NOT sent to the
    /// host over the agent connection — that connection authenticated with
    /// the token being replaced — so the operator must hand it to the host
    /// out of band (`fleet-hub agent-token`, then `fleet-agent install`).
    pub const E_AGENT_REINSTALL: &str = "E_AGENT_REINSTALL";
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
    /// The PTY is not accepting input: the writer thread's bounded queue is
    /// full because the attached process stopped reading.
    pub const E_PTY_BUSY: &str = "E_PTY_BUSY";
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
    /// Workspace repair: the project's main checkout is missing or not a git
    /// repository. Never faked with `mkdir`; restore or re-clone it.
    pub const E_REPO_MISSING: &str = "E_REPO_MISSING";
    /// Workspace repair: the worktree's branch is checked out in the main
    /// checkout, so git refuses a second checkout and we refuse to hijack it.
    pub const E_BRANCH_CHECKED_OUT: &str = "E_BRANCH_CHECKED_OUT";
    /// Workspace repair: the worktree is locked (`git worktree lock`) and its
    /// directory is gone; unlock it to allow the repair.
    pub const E_WORKSPACE_LOCKED: &str = "E_WORKSPACE_LOCKED";
    /// Workspace repair: a git step failed, the result did not verify, or the
    /// directory is not a worktree and not empty (never deleted automatically).
    pub const E_REPAIR_FAILED: &str = "E_REPAIR_FAILED";
    /// Workspace repair: an automatic check (new session, restart, recreate)
    /// found a problem only an explicit repair may fix (unregister a stale
    /// entry, adopt a moved checkout, recreate a branch, re-link). Run
    /// Repair workspace.
    pub const E_REPAIR_REQUIRED: &str = "E_REPAIR_REQUIRED";
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
    /// An address parsed, but names no participant in this fleet.
    pub const E_PARTICIPANT_UNKNOWN: &str = "E_PARTICIPANT_UNKNOWN";
    /// The participant resolved, but is tombstoned — the endpoint is gone.
    pub const E_PARTICIPANT_RETIRED: &str = "E_PARTICIPANT_RETIRED";
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
    /// A tracker (Jira …) refused or could not be reached; `details.state`
    /// says how (auth_failed | rate_limited | unreachable | captcha |
    /// unconfigured). The cache still answers; this is only a live call.
    pub const E_TRACKER: &str = "E_TRACKER";
    /// `move_session` with `strict: true` only: the source worktree has
    /// uncommitted changes (`details.dirty_files`). Without `strict` a move
    /// carries them instead of refusing.
    pub const E_MOVE_DIRTY: &str = "E_MOVE_DIRTY";
    /// `move_session` with `strict: true` only: the branch is not on origin,
    /// or has commits origin lacks. Without `strict` a move carries them
    /// instead of refusing; a move never pushes either way.
    pub const E_MOVE_UNPUSHED: &str = "E_MOVE_UNPUSHED";
    /// `move_session`: the transcript is over `move.max_transcript_mb`, or
    /// the git bundle of the carried work is over `move.max_bundle_mb`
    /// (`details.bytes`, `details.cap_bytes`; `details.payload` is
    /// `"transcript"` or `"bundle"`).
    pub const E_MOVE_TOO_LARGE: &str = "E_MOVE_TOO_LARGE";
    /// `move_session`: a step failed after the target session was started;
    /// both sessions were left running (`details.target_session_id`).
    pub const E_MOVE_PARTIAL: &str = "E_MOVE_PARTIAL";
    /// `move_session`: the source worktree is mid merge / rebase /
    /// cherry-pick / revert / bisect (`details.operation`); finish or abort it.
    pub const E_MOVE_MIDOP: &str = "E_MOVE_MIDOP";
    /// `move_session`: the target worktree has uncommitted changes — its
    /// own, work carried by an earlier move that did not finish, or the copy
    /// left behind when this session was moved away from that host (a move
    /// carries the work, it does not remove it from the source); the move
    /// never overwrites them and the source is not touched.
    pub const E_MOVE_TARGET_DIRTY: &str = "E_MOVE_TARGET_DIRTY";
    /// `move_session`: carrying the work failed before the target started
    /// (`details.step`, `details.stderr`, and `details.cause_code` when the
    /// transport itself failed); the source is untouched.
    pub const E_MOVE_CARRY: &str = "E_MOVE_CARRY";
    /// No Claude transcript was found for the session (it has not written a
    /// turn yet, or runs on another cwd), or it is empty.
    pub const E_NO_TRANSCRIPT: &str = "E_NO_TRANSCRIPT";
    /// The call needs a desktop confirmation first (`mcp.confirm_destructive`);
    /// retry with the `confirm_nonce` from `details` once approved.
    pub const E_CONFIRM_REQUIRED: &str = "E_CONFIRM_REQUIRED";
    /// Asset catalog: no catalog repo is configured, or it is configured but
    /// not loaded yet (`catalog_configure` / `catalog_load` first).
    pub const E_CATALOG_NOT_CONFIGURED: &str = "E_CATALOG_NOT_CONFIGURED";
    /// Asset catalog: a git operation on the catalog repo (clone, pull,
    /// `rev-parse HEAD`) failed. Distinct from `E_GIT`, which covers a
    /// session's worktree.
    pub const E_CATALOG_GIT: &str = "E_CATALOG_GIT";
    /// Asset catalog: an asset file in the repo could not be parsed into the
    /// IR (bad front-matter, unknown kind, duplicate name).
    pub const E_CATALOG_PARSE: &str = "E_CATALOG_PARSE";
    /// Asset catalog: the harness cannot express this asset kind.
    pub const E_ASSET_UNSUPPORTED: &str = "E_ASSET_UNSUPPORTED";
    /// Asset catalog: the import would overwrite an asset already in the
    /// repo. The importer never overwrites.
    pub const E_ASSET_EXISTS: &str = "E_ASSET_EXISTS";
    /// Asset catalog: no asset of that kind and name is in the catalog.
    pub const E_ASSET_NOT_FOUND: &str = "E_ASSET_NOT_FOUND";
    /// Asset catalog: an authoring save was refused because the asset has
    /// lint errors. `details` carries the whole `LintReport` (errors and
    /// warnings); nothing was written to the repo.
    pub const E_LINT: &str = "E_LINT";
    /// Asset catalog: a host scan failed or returned unusable output. The
    /// scan fails closed — a partial snapshot is never treated as empty.
    pub const E_SCAN: &str = "E_SCAN";
    /// Asset sync: the plan id handed to `sync_apply` is unknown or has
    /// expired out of the in-process plan registry. Re-plan and apply again.
    pub const E_SYNC_PLAN_STALE: &str = "E_SYNC_PLAN_STALE";
    /// Asset sync: the plan contains actions blocked on `${NAME}` secrets
    /// with no value. The message lists the NAMES only, never a value. Set
    /// them (`catalog_set_secret`) or re-apply with `force_partial`.
    pub const E_SECRET_MISSING: &str = "E_SECRET_MISSING";
    /// Remote (hub-client) mode: the hub answered `401`. The client token is
    /// no longer accepted — the operator revoked it, or the hub was re-inited
    /// with a fresh database. The desktop must send the user back to the Hub
    /// settings to pair again; retrying cannot help.
    pub const E_UNAUTHORIZED: &str = "E_UNAUTHORIZED";
    /// Remote (hub-client) mode: the hub could not be reached or did not
    /// answer usefully (connection refused, DNS, timeout, a proxy's 5xx). The
    /// app shows the last snapshot and a banner; it never falls back to
    /// managing the fleet itself, which would make two brains for one fleet.
    pub const E_HUB_UNREACHABLE: &str = "E_HUB_UNREACHABLE";
    /// Remote (hub-client) mode: the hub took the request and did not answer
    /// within the client's bound. Unlike `E_HUB_UNREACHABLE`, the operation
    /// may have run — a mutation must not be blindly retried; re-list first.
    pub const E_HUB_TIMEOUT: &str = "E_HUB_TIMEOUT";
    /// Remote (hub-client) mode: a hub is configured, but this launch could
    /// not use it — no stored client token, a keychain that would not open,
    /// plain `http://` without the opt-in, a URL that does not parse. The app
    /// then owns nothing and refuses every fleet command with this code and
    /// the reason, rather than quietly managing the hub's fleet itself. The
    /// fix is in Settings → Hub: pair again, or Disconnect.
    pub const E_HUB_UNAVAILABLE: &str = "E_HUB_UNAVAILABLE";
    /// Remote (hub-client) mode: the hub's `/events` hello frame named a
    /// wire-contract revision outside the range this build reads
    /// (`MIN_HUB_CONTRACT..=MAX_HUB_CONTRACT`,
    /// `src-tauri/src/backend/contract.rs`), so a renamed field in its
    /// answers would arrive as a silent default rather than as an error. The
    /// desktop refuses every call to that hub — read and mutation alike —
    /// until one side is updated, and the event bridge applies no row event
    /// from it either. The message names which side is behind, the same way
    /// the banner does; `details` carries the hub's revision and the bound it
    /// missed.
    pub const E_HUB_CONTRACT: &str = "E_HUB_CONTRACT";
    /// Remote (hub-client) mode: the hub answered the call with a JSON-RPC
    /// *protocol* error rather than a tool result — it serves no tool by that
    /// name, or could not bind the arguments. That is the two sides
    /// disagreeing about what the API is, which in practice means a hub older
    /// than this app: a tool added here is not on it yet.
    ///
    /// Its own code, and not `E_INTERNAL`, because it is the one failure a
    /// caller can reasonably *degrade* past. A feature built on a new tool
    /// can fall back to what it did before this code, where it must not do
    /// so for an ordinary internal error.
    pub const E_HUB_PROTOCOL: &str = "E_HUB_PROTOCOL";
    /// Pairing: the hub URL is plain `http://` to a host that is not
    /// loopback, so the client token this pairing is about to mint — a
    /// credential for the whole fleet — would cross the network in the clear
    /// on every call, forever.
    ///
    /// Its own code because it is the one pairing failure the user can
    /// *decide* their way past: the message names the risk and the dialog
    /// offers to send it anyway (the client half of the hub's own
    /// `--allow-plaintext`). Every other pairing failure is something to fix,
    /// not something to accept.
    pub const E_HUB_PLAINTEXT: &str = "E_HUB_PLAINTEXT";
    /// Remote (hub-client) mode: the command only makes sense against a fleet
    /// this process owns, and no hub tool does it. Three families — things
    /// about *this machine* (the PTY, SSH tunnels, the local prerequisites
    /// check), fleet administration a paired client is refused by design
    /// (`add_host`, `provision_hosts`, secrets, asset sync), and asset-catalog
    /// authoring, which edits a git checkout only the owning machine has.
    ///
    /// The message always names what to do instead: run it on the hub, or in
    /// a standalone app. Never returned in standalone mode.
    pub const E_LOCAL_ONLY: &str = "E_LOCAL_ONLY";

    /// Every code a *transport* raises when a command did not reach, or did
    /// not come back from, the host — over SSH or over an agent.
    ///
    /// The single list the service layer branches on, so the two transports
    /// cannot drift apart: `E_AGENT_OFFLINE` must not fall through a branch
    /// that `E_SSH` is caught by. Codes about the *work* (`E_REPO_MISSING`,
    /// `E_TMUX`…) are deliberately absent.
    ///
    /// **`E_TIMEOUT` is deliberately absent too.** Both transports report a
    /// blown wall clock as `E_SSH_TIMEOUT` (`ssh::wall_clock_error`, and the
    /// agent registry matches it on purpose), so the `E_TIMEOUT`s that reach
    /// the service layer come from *local* deadlines — `HostExec::run_bash`'s
    /// local branch, `add_project`'s local script — where "the host is
    /// unreachable" would be a lie. Branches that mean "the command may have
    /// been cut off" rather than "the transport failed" list it themselves;
    /// see [`may_have_run`].
    pub const TRANSPORT_FAILURES: [&str; 4] =
        [E_SSH, E_SSH_TIMEOUT, E_AGENT_OFFLINE, E_AGENT_PROTOCOL];

    /// Did the transport, rather than the command, fail? See
    /// [`TRANSPORT_FAILURES`].
    pub fn is_transport_failure(code: &str) -> bool {
        TRANSPORT_FAILURES.contains(&code)
    }

    /// The transport failures under which the command **may already have
    /// run**: it was sent and its outcome is unknown, rather than never
    /// having left. `E_SSH` and `E_AGENT_OFFLINE` are the two that mean no
    /// connection, so they are excluded — a caller doing something
    /// non-idempotent (`gh repo create`, a billed usage request) can safely
    /// treat those as "nothing happened" and retry elsewhere.
    pub fn may_have_run(code: &str) -> bool {
        matches!(code, E_SSH_TIMEOUT | E_TIMEOUT | E_AGENT_PROTOCOL)
    }
}

#[derive(Debug, Serialize)]
pub struct IpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

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

/// Lock an app-level mutex (the `Store` first of all), mapping a poisoned
/// lock to `E_LOCK`. `let s = lock(store)?;` is the one idiom every
/// service / command / MCP tool uses; MCP sites chain `.map_err(to_mcp_err)`.
pub fn lock<T>(m: &std::sync::Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, IpcError> {
    m.lock().map_err(|_| IpcError::lock())
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
