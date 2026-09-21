//! The UX agent's operator session: who it is, and the one rule that keeps it
//! from acting on itself.
//!
//! This is the ONLY module that knows the operator is special. Everything
//! else treats it as an ordinary session — which is the point: it gets the
//! transcript, the conversation view, restart, reboot survival and the
//! sidebar for free, and removing the feature means deleting this file.

use crate::cancel::CancellationRegistry;
use crate::ipc_error::{codes, lock, IpcError};
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use std::sync::{Arc, Mutex};

/// `settings` key holding `"<host_alias>/<tmux_name>"` for the live operator
/// session. Absent until `ensure_operator` has run once.
pub const SETTING_OPERATOR_SESSION: &str = "operator.session";

/// Where the operator session lives. Identity is `(host, tmux name)` rather
/// than a row id because ids churn on re-discovery, exactly as the quick
/// switcher's MRU key does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorRef {
    pub host_alias: String,
    pub tmux_name: String,
}

/// PURE: render a reference for storage.
pub fn format_ref(r: &OperatorRef) -> String {
    format!("{}/{}", r.host_alias, r.tmux_name)
}

/// PURE: read a stored reference. Both halves must be non-empty — a
/// half-written value must resolve to "no operator", never to a reference
/// that guards the wrong session (or every session on a host).
pub fn parse_ref(raw: &str) -> Option<OperatorRef> {
    let (host, tmux) = raw.split_once('/')?;
    if host.is_empty() || tmux.is_empty() {
        return None;
    }
    Some(OperatorRef {
        host_alias: host.to_string(),
        tmux_name: tmux.to_string(),
    })
}

/// The recorded operator, if one has ever been created. A store error reads
/// as "none": the guard's job is to refuse acting on a KNOWN operator, and a
/// database that cannot answer has not named one.
pub fn operator_ref(store: &Store) -> Option<OperatorRef> {
    store
        .get_setting(SETTING_OPERATOR_SESSION)
        .ok()
        .flatten()
        .as_deref()
        .and_then(parse_ref)
}

/// Record the operator's whereabouts. Called once by `ensure_operator`.
pub fn set_operator_ref(store: &Store, r: &OperatorRef) -> Result<(), IpcError> {
    store
        .set_setting(SETTING_OPERATOR_SESSION, &format_ref(r))
        .map_err(|e| IpcError::new(codes::E_SQLITE, format!("record the operator session: {e}")))
}

/// Refuse a session-addressed operation aimed at the operator itself.
///
/// Without this, "tidy up the zombie sessions" ends the conversation that
/// asked for it — mid-sentence, with no one left to say what happened.
///
/// **Who calls it, and who deliberately does not.** `kill_session`,
/// `safe_kill_session`, `move_session`, `rename_session` and
/// `recreate_session` all ask first;
/// `tests::every_session_addressed_operation_asks_the_guard_first` fails if
/// any of them stops. Two omissions are on purpose:
///
/// - **`restart_session` is EXEMPT.** It is the panel's own `lost` recovery
///   (`restartOperator()` in `src/lib/operator.ts`), so guarding it would
///   make the agent refuse the one button that brings it back. It is also
///   not destructive: the row, the transcript and the conversation survive a
///   restart. `tests::restart_session_is_deliberately_exempt_from_the_guard`
///   pins this so the omission cannot be read as an oversight again.
/// - **`repair_session` is `confirm: true`**, so a human stands between the
///   agent and the operator already.
///
/// **Why `rename_session` is guarded rather than the identity moved off the
/// name.** A rename does not merely dodge the guard once — [`OperatorRef`]
/// is `(host, tmux name)`, so a rename destroys the identity permanently:
/// the guard then matches nothing, `operator_status` reports `absent`, and
/// the next press mints a fresh token (revoking the running agent's) and
/// calls `new_session` on a name the renamed session may still hold. Keying
/// on the session row id instead would only move the problem: ids churn on
/// re-discovery, which is why the reference is a name in the first place.
/// Refusing the rename keeps one identity with one failure mode, and costs
/// nothing real — the operator's fixed name is what makes it recognisable in
/// `tmux ls`, and a friendly label is still free to change.
pub fn refuse_if_operator(
    store: &Store,
    host_alias: &str,
    tmux_name: &str,
    what: &str,
) -> Result<(), IpcError> {
    match operator_ref(store) {
        Some(r) if r.host_alias == host_alias && r.tmux_name == tmux_name => Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "{what} refused: {tmux_name} on {host_alias} is the UX agent's own session. \
                 Close the agent panel and act on it from the sidebar if you mean it."
            ),
        )),
        _ => Ok(()),
    }
}

/// Owner/repo of the operator's own project row. Not a real repository —
/// `upsert_system_project` flags it so the picker hides it and the sweep
/// leaves it be.
pub const OPERATOR_OWNER: &str = "fleet";
pub const OPERATOR_REPO: &str = "operator";
/// The operator's working directory. Deliberately NOT one of the user's
/// checkouts: the agent runs sessions, it does not write code.
pub const OPERATOR_DIR: &str = "~/.claude-fleet/operator";
/// Fixed tmux name, so the session is recognisable in the sidebar and in
/// `tmux ls` without consulting the database.
pub const OPERATOR_TMUX_NAME: &str = "fleet-operator";
/// The operator always runs on the machine that serves the control API —
/// `local` for the desktop, the hub's own host for a hub. That is why the
/// endpoint below can be loopback and no public URL is ever baked into the
/// file.
pub const OPERATOR_HOST: &str = "local";
/// Name of the operator's client-token row. A client token in mode `full`,
/// never the master token: the agent is a paired client like any other, and
/// revoking it by name is how the operator is disarmed.
pub const OPERATOR_CLIENT_NAME: &str = "ux-agent";
/// The file Claude Code reads a PROJECT's MCP servers from. Not
/// `.claude/settings.json`, which is where `provision.rs` installs hooks and
/// where an `mcpServers` block would be silently ignored. Nothing else in
/// the fleet writes this path, so there is no merge race with provisioning.
pub const OPERATOR_MCP_FILE: &str = ".mcp.json";
/// `settings` key holding the SHA-256 of the operator's client token, so
/// `operator_status` can tell a revoked token from a healthy one without
/// reading the secret back off the host.
pub const SETTING_OPERATOR_TOKEN_SHA: &str = "operator.token_sha";

/// PURE: the operator's standing instructions.
pub fn claude_md() -> &'static str {
    "# You are the fleet operator\n\
     \n\
     You drive claude-fleet through its MCP control API on behalf of the\n\
     person at the keyboard. `list_sessions`, `fleet_health` and\n\
     `session_conversation` tell you what is happening; `send_prompt`,\n\
     `new_session` and the rest change it.\n\
     \n\
     Two rules.\n\
     \n\
     1. **Destructive work is proposed, never assumed.** Killing, deleting,\n\
     moving, broadcasting and committing stop for a confirmation you do not\n\
     control. Say plainly what you are about to do and let the dialog do its\n\
     job; do not try to route around a refusal.\n\
     \n\
     2. **This directory is not a repository and you do not write code in\n\
     it.** When work needs doing in a project, start a session on the right\n\
     host and brief it. You are the operator, not the worker.\n\
     \n\
     Answer in the language the person uses. Prefer one short paragraph over\n\
     a report: the sidebar already shows what changed.\n"
}

/// PURE: the operator's `.mcp.json`, pointing its MCP client at this fleet
/// with its own bearer token.
///
/// `.mcp.json` and not `.claude/settings.json`: settings.json is where
/// `provision.rs` installs HOOKS, while MCP servers are read from
/// `~/.claude.json` globally or from a project's `.mcp.json`. Writing
/// `mcpServers` into settings.json would be silently ignored and the
/// operator would fall back to the host token already in `~/.claude.json` —
/// which is bound to its own host and refuses other hosts' sessions,
/// defeating the point of the agent having a fleet-wide, independently
/// revocable identity. Project-scoped rather than a merge into
/// `~/.claude.json` so the token is the operator's alone and not handed to
/// every session on the machine.
pub fn mcp_json(endpoint: &str, token: &str) -> String {
    serde_json::json!({
        "mcpServers": {
            "claude-fleet": {
                "type": "http",
                "url": endpoint,
                "headers": { "Authorization": format!("Bearer {token}") }
            }
        }
    })
    .to_string()
}

/// The part of `ensure_operator` that reaches outside the database: where
/// the operator's directory actually is on the host, the two files that go
/// into it, and starting the session.
///
/// It is a trait because `new_session` has no exec seam of its own — the
/// live path writes a bearer token into `$HOME` and runs `tmux new-session`,
/// neither of which a unit test may do (and macOS CI has no tmux at all).
/// Everything the idempotence test cares about is store state, so the double
/// in the test module produces that and nothing else.
#[async_trait::async_trait]
pub(crate) trait OperatorHost: Send + Sync {
    /// The operator's working directory as an ABSOLUTE path on the host.
    ///
    /// This is not a formality. `new_session` hands the project's
    /// `base_path` straight to `tmux new-session -c <path>`, which is an
    /// argv element with no shell behind it, so a literal `~` would be taken
    /// as a directory of that name rather than the home directory. The
    /// provisioning helpers expand `~` themselves (`remote_path` /
    /// `expand_home_local`); the session lifecycle does not, so the path
    /// stored on the project row has to be resolved here first.
    async fn resolve_dir(&self) -> Result<String, IpcError>;

    /// Write `CLAUDE.md` and [`OPERATOR_MCP_FILE`] under `dir`.
    async fn write_files(&self, dir: &str, claude_md: &str, mcp_json: &str)
        -> Result<(), IpcError>;

    /// Start the operator's session and return its row.
    async fn start_session(&self, project_id: i64) -> Result<SessionRow, IpcError>;
}

/// The live host side: the provisioning helpers and the ordinary session
/// lifecycle, on the machine that serves the control API.
struct LiveHost {
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    reg: Arc<CancellationRegistry>,
}

#[async_trait::async_trait]
impl OperatorHost for LiveHost {
    async fn resolve_dir(&self) -> Result<String, IpcError> {
        crate::service::provision::expand_home_local(OPERATOR_DIR)
    }

    async fn write_files(
        &self,
        dir: &str,
        claude_md: &str,
        mcp_json: &str,
    ) -> Result<(), IpcError> {
        crate::service::provision::write_host_file(
            self.ssh.as_ref(),
            OPERATOR_HOST,
            dir,
            &format!("{dir}/CLAUDE.md"),
            claude_md,
        )
        .await?;
        // `.mcp.json` carries the bearer token, so it goes through the
        // secret path: never in an argv, never a truncated file on a failed
        // write, and 0600 on disk.
        crate::service::provision::write_host_file_secret(
            self.ssh.as_ref(),
            OPERATOR_HOST,
            dir,
            &format!("{dir}/{OPERATOR_MCP_FILE}"),
            mcp_json,
        )
        .await
    }

    async fn start_session(&self, project_id: i64) -> Result<SessionRow, IpcError> {
        // An ordinary work session, which is the whole point: transcript,
        // conversation view, restart and reboot survival all come for free.
        crate::service::sessions::new_session(
            crate::service::sessions::NewSessionArgs {
                host_alias: OPERATOR_HOST.to_string(),
                project_id,
                worktree_id: None,
                name: OPERATOR_TMUX_NAME.to_string(),
                call_id: None,
                new_worktree: None,
                base_branch: None,
                kind: Some("work".to_string()),
                start_command: None,
                friendly_name: Some("fleet operator".to_string()),
            },
            &self.store,
            &self.ssh,
            &self.reg,
        )
        .await
    }
}

/// What the FAB needs to know before it opens a panel.
///
/// Deliberately plain fields, always serialised (no `#[serde(default)]`, no
/// `skip_serializing_if`): a hub-read result must deserialise without serde
/// defaults, so both `Option` fields have to be present on the wire even
/// when `null`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OperatorStatus {
    pub ready: bool,
    pub session: Option<SessionRow>,
    /// `null`, or one of `"absent"`, `"lost"`, `"no_mcp"`, `"token_revoked"`.
    pub blocked: Option<String>,
}

/// Why the agent can or cannot work right now.
///
/// The control-API check comes FIRST, before whether the operator has ever
/// been created — not the other way round. The caller (the FAB) only calls
/// `ensure_operator` in response to `"absent"`, and `ensure_operator` cannot
/// do useful work without the control API: `configured_port` refuses
/// (`E_PROVISION`) whenever the API has never been enabled, since enabling
/// it is what mints the master token in the first place
/// (`src-tauri/src/bootstrap/mcp.rs`). If `absent` were reported while the
/// API is off, the button would call `ensure_operator` and get an opaque
/// `E_PROVISION` back — the exact "panel that apologises" this design
/// exists to avoid. Worse, a desktop where someone merely opened the
/// control-API settings panel once already has a minted token
/// (`src-tauri/src/commands/mcp.rs`) with the API still off; under
/// absent-first `ensure_operator` would then SUCCEED, birthing a session
/// whose `.mcp.json` points at a loopback port nothing is listening on — a
/// toolless agent that looks healthy, which is worse than one that visibly
/// cannot be created. Checking `enabled` first turns both of those into the
/// same actionable `"no_mcp"` answer, and loses nothing: a fleet with the
/// API on and no agent still reports `"absent"`, which is the only case
/// that matters for the button. The control-API check is computed HERE,
/// inside the authoritative backend, rather than from the desktop's
/// `mcp_status` — that command is `LocalOnly`, and a hub's API is always
/// on. Asking the backend that would actually serve the agent is the same
/// question in both modes.
pub fn operator_status(store: &Mutex<Store>) -> Result<OperatorStatus, IpcError> {
    let s = lock(store)?;
    let blocked = |why: &str, session: Option<SessionRow>| OperatorStatus {
        ready: false,
        session,
        blocked: Some(why.to_string()),
    };

    if !crate::mcp::settings::McpSettings::read(&s)?.enabled {
        return Ok(blocked("no_mcp", None));
    }
    let Some(r) = operator_ref(&s) else {
        return Ok(blocked("absent", None));
    };
    let row = s
        .get_session(&r.tmux_name, &r.host_alias)
        .map_err(|e| IpcError::new(codes::E_SQLITE, format!("find operator: {e}")))?;
    let Some(row) = row else {
        return Ok(blocked("absent", None));
    };
    if row.lost_at.is_some() {
        return Ok(blocked("lost", Some(row)));
    }
    // A revoked token is a deliberate act, so nothing re-mints itself — the
    // panel offers a button and the person presses it.
    let sha = s.get_setting(SETTING_OPERATOR_TOKEN_SHA).ok().flatten();
    let live = s
        .active_client_tokens()
        .map(|rows| rows.iter().any(|t| Some(&t.token_sha256) == sha.as_ref()))
        .unwrap_or(false);
    if !live {
        return Ok(blocked("token_revoked", Some(row)));
    }
    Ok(OperatorStatus {
        ready: true,
        session: Some(row),
        blocked: None,
    })
}

/// Make sure the operator session exists, and return its row.
///
/// Idempotent by design — this runs on every press of the FAB. When a live
/// session is already recorded it is a pair of store reads and nothing else.
/// Birth is lazy for exactly this reason: an agent you never open costs
/// nothing.
///
/// Idempotent CONCURRENTLY as well as sequentially: overlapping calls are
/// serialised by [`operator_birth_lock`], which is what makes a double-click
/// on the button harmless rather than the way the agent loses its identity.
pub async fn ensure_operator(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<SessionRow, IpcError> {
    let host = LiveHost {
        store: Arc::clone(store),
        ssh: Arc::clone(ssh),
        reg: Arc::clone(reg),
    };
    ensure_operator_on(store, &host).await
}

type BirthLocks = Mutex<std::collections::HashMap<usize, Arc<tokio::sync::Mutex<()>>>>;

/// The one-birth-at-a-time lock for a store, created on first use.
///
/// [`ensure_operator_on`] mints a token, revokes the previous `ux-agent` row
/// and hands the secret to the host across several store guards and three
/// `.await`s. Two callers interleaving there — an ordinary double-click on
/// the FAB, or the desktop and a phone pressing at once — each revoke the
/// other's token. The store block and the `.mcp.json` write are separately
/// ordered, so whichever store block commits last decides what the database
/// vouches for while whichever `write_files` lands last decides what the
/// host holds. When those disagree, every MCP call the agent makes 401s
/// while `operator_status` still answers `ready` (the sha it compares is the
/// live row's) — and guard #1 short-circuits every later press, so nothing
/// re-mints. The only way out is killing the session by hand.
///
/// **Why an async mutex and not a `MoveClaim`-style refusal.** A claim would
/// close the hole, but it would hand the second press an `E_INVALID_STATE`
/// for doing nothing wrong, and the panel renders a failed `ensure_operator`
/// as `absent` — a button that says "the agent is not running" because you
/// pressed it twice. Waiting closes the hole AND keeps the promise this
/// function's own doc comment makes: the second caller runs guard #1 after
/// the first has finished, finds the session it just created, and returns
/// that row. The idempotence becomes true concurrently, not only
/// sequentially. The wait is bounded by the birth itself, which is already
/// what the caller is waiting for.
///
/// Keyed by store address, like `move_session`'s `moves_in_flight`: one
/// store per app, and the address keeps parallel tests on separate in-memory
/// stores from serialising against each other. The map holds one `Arc` per
/// store ever seen — a few bytes, bounded by the number of stores the
/// process creates.
fn operator_birth_lock(store: &Mutex<Store>) -> Result<Arc<tokio::sync::Mutex<()>>, IpcError> {
    static LOCKS: std::sync::OnceLock<BirthLocks> = std::sync::OnceLock::new();
    let map = LOCKS.get_or_init(Default::default);
    let mut g = map.lock().map_err(|_| IpcError::lock())?;
    Ok(Arc::clone(
        g.entry(store as *const Mutex<Store> as usize).or_default(),
    ))
}

/// [`ensure_operator`] with the host side injected. The store guard is taken
/// in short scoped blocks below and dropped before every `.await`:
/// this function interleaves database work with SSH and tmux work, and
/// holding the mutex across either would block every other command in the
/// app for the length of a network round trip.
pub(crate) async fn ensure_operator_on(
    store: &Arc<Mutex<Store>>,
    host: &dyn OperatorHost,
) -> Result<SessionRow, IpcError> {
    // 0. One birth at a time. Everything below — guard #1 included — runs
    //    inside this, so a second press that arrives mid-birth waits and
    //    then finds the session the first one created. See
    //    [`operator_birth_lock`].
    let birth = operator_birth_lock(store)?;
    let _birth = birth.lock().await;

    // 1. Already alive? Then there is nothing to do. (guard #1)
    {
        let s = lock(store)?;
        if let Some(r) = operator_ref(&s) {
            if let Some(row) = s
                .get_session(&r.tmux_name, &r.host_alias)
                .map_err(|e| IpcError::new(codes::E_SQLITE, format!("find operator: {e}")))?
            {
                if row.lost_at.is_none() {
                    return Ok(row);
                }
            }
        }
    }

    // 2. The endpoint, BEFORE anything is mutated anywhere. (guard #2)
    //    `configured_port` refuses when the control API has never been
    //    enabled, and that refusal has to be the first thing that can
    //    happen: with it below the token work, every press on such a fleet
    //    would revoke the live token, insert an undeliverable replacement
    //    and record its hash, and only then fail. Loopback in both modes —
    //    the operator runs on the machine that serves the control API, so no
    //    public URL is ever baked into the file it is about to be handed.
    //    The control API is NOT enabled here: that is the user's decision,
    //    not something to do behind their back.
    let endpoint = {
        let s = lock(store)?;
        format!(
            "http://127.0.0.1:{}/mcp",
            crate::mcp::settings::configured_port(&s)?
        )
    };

    // The absolute directory has to be known before the project row is
    // written, because `base_path` is what the session's pane starts in —
    // see [`OperatorHost::resolve_dir`].
    let dir = host.resolve_dir().await?;

    // 3. The project row, and a token that is NOT yet persisted. (#3)
    //    Only the hash is ever stored; the plaintext lives in the operator's
    //    `.mcp.json` on the host and nowhere else.
    let (project_id, token) = {
        let s = lock(store)?;
        let project_id = s
            .upsert_system_project(OPERATOR_OWNER, OPERATOR_REPO, &dir)
            .map_err(|e| IpcError::new(codes::E_SQLITE, format!("operator project row: {e}")))?;
        (project_id, crate::mcp::generate_token())
    };

    // 4. The files on the host FIRST, the token committed only once the host
    //    really has it. This is `provision::resolve_host_token` /
    //    `commit_host_token`'s rule — "a failed provision never strands a
    //    host on a token it never received" — and this is the only other
    //    place in the codebase that hands a host a secret. The old order
    //    (commit, then deliver) self-healed only because guard #1
    //    short-circuits on a LIVE recorded session and a failed birth
    //    records none; that is a property of a guard twenty lines away, not
    //    of this step, and it is the same weakness the birth lock above
    //    closes against a race.
    host.write_files(&dir, claude_md(), &mcp_json(&endpoint, &token))
        .await?;

    // 5. Commit the token. (guard #4)
    {
        let s = lock(store)?;
        let sha = crate::mcp::auth::sha256_hex(&token);
        // A previous token by this name may still be live (a half-finished
        // birth, or a session that was lost). Revoke it rather than colliding
        // on the partial unique index over live `client_tokens(name)`.
        // `E_NOTFOUND` here just means there was none.
        let _ = s.revoke_client_token(OPERATOR_CLIENT_NAME);
        s.insert_client_token(OPERATOR_CLIENT_NAME, &sha, "full")?;
        s.set_setting(SETTING_OPERATOR_TOKEN_SHA, &sha)
            .map_err(|e| {
                IpcError::new(codes::E_SQLITE, format!("record the operator token: {e}"))
            })?;
    }

    // 6. The session itself.
    let row = host.start_session(project_id).await?;

    // 7. Record where it lives, so the self-guard and the next press can
    //    find it. (guard #5)
    {
        let s = lock(store)?;
        set_operator_ref(
            &s,
            &OperatorRef {
                host_alias: row.host_alias.clone(),
                tmux_name: row.tmux_name.clone(),
            },
        )?;
    }
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_reference_round_trips_and_a_malformed_one_is_no_reference() {
        let r = OperatorRef {
            host_alias: "mefistos".into(),
            tmux_name: "fleet-operator".into(),
        };
        assert_eq!(format_ref(&r), "mefistos/fleet-operator");
        assert_eq!(parse_ref("mefistos/fleet-operator"), Some(r));
        // A tmux name may not contain '/', so the first separator is the only
        // one — but a value with no separator, or an empty half, is not a
        // reference and must not resolve to one.
        assert_eq!(parse_ref("mefistos"), None);
        assert_eq!(parse_ref("/fleet-operator"), None);
        assert_eq!(parse_ref("mefistos/"), None);
        assert_eq!(parse_ref(""), None);
    }

    #[test]
    fn an_unrecorded_operator_guards_nothing() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(operator_ref(&s), None);
        refuse_if_operator(&s, "local", "anything", "kill_session")
            .expect("with no operator recorded, nothing is refused");
    }

    #[test]
    fn the_operator_refuses_to_be_acted_on_and_its_neighbours_do_not() {
        let s = Store::open_in_memory().unwrap();
        let r = OperatorRef {
            host_alias: "local".into(),
            tmux_name: "fleet-operator".into(),
        };
        set_operator_ref(&s, &r).unwrap();
        assert_eq!(operator_ref(&s), Some(r));

        let err = refuse_if_operator(&s, "local", "fleet-operator", "kill_session")
            .expect_err("the operator must refuse to be killed");
        assert_eq!(err.code, crate::ipc_error::codes::E_FORBIDDEN);
        assert!(
            err.message.contains("kill_session"),
            "the refusal names what was attempted: {}",
            err.message
        );

        // Same name on another host, and another name on the same host, are
        // ordinary sessions.
        refuse_if_operator(&s, "mefistos", "fleet-operator", "kill_session").unwrap();
        refuse_if_operator(&s, "local", "blue-sirius", "kill_session").unwrap();
    }

    #[test]
    fn the_mcp_file_points_at_the_endpoint_and_carries_the_token() {
        let j = mcp_json("http://127.0.0.1:4180/mcp", "deadbeef");
        let v: serde_json::Value = serde_json::from_str(&j).expect("valid JSON");
        let srv = &v["mcpServers"]["claude-fleet"];
        assert_eq!(srv["type"], "http");
        assert_eq!(srv["url"], "http://127.0.0.1:4180/mcp");
        assert_eq!(srv["headers"]["Authorization"], "Bearer deadbeef");
    }

    #[test]
    fn the_operating_instructions_state_the_two_rules_that_matter() {
        let md = claude_md();
        assert!(
            md.contains("propose") || md.contains("confirm"),
            "the operator must be told destructive work is confirmed, not assumed"
        );
        assert!(
            md.contains("not a repository") || md.contains("do not write code"),
            "the operator must be told its directory is not a place to write code"
        );
    }

    // ------------------------------------------------------- ensure_operator

    /// The triple every `ensure_operator` test needs: a store with the
    /// `local` host row and a master token (without one `configured_port`
    /// refuses), plus the ssh client and cancellation registry the real
    /// entry point takes.
    fn fixture() -> (
        Arc<Mutex<Store>>,
        Arc<crate::ssh::SshClient>,
        Arc<crate::cancel::CancellationRegistry>,
    ) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host(OPERATOR_HOST).unwrap();
        s.set_setting(crate::mcp::SETTING_TOKEN, "master-token")
            .unwrap();
        (
            Arc::new(Mutex::new(s)),
            Arc::new(crate::ssh::SshClient::new()),
            crate::cancel::CancellationRegistry::new(),
        )
    }

    /// The host side, faked. The real one writes a bearer token into `$HOME`
    /// and starts a tmux session; neither belongs in a unit test, and macOS
    /// CI has no tmux at all. Everything the test asserts on — the project
    /// row, the token row, the recorded reference, the returned session — is
    /// store state, which this double produces exactly as the live one does.
    struct FakeHost {
        store: Arc<Mutex<Store>>,
        dir: String,
        files: std::sync::Mutex<Vec<(String, String)>>,
        starts: std::sync::atomic::AtomicUsize,
    }

    impl FakeHost {
        fn new(store: &Arc<Mutex<Store>>) -> Self {
            Self {
                store: Arc::clone(store),
                dir: "/tmp/fleet-operator-test".to_string(),
                files: std::sync::Mutex::new(Vec::new()),
                starts: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl OperatorHost for FakeHost {
        async fn resolve_dir(&self) -> Result<String, IpcError> {
            Ok(self.dir.clone())
        }
        async fn write_files(
            &self,
            dir: &str,
            claude_md: &str,
            mcp_json: &str,
        ) -> Result<(), IpcError> {
            let mut f = self.files.lock().unwrap();
            f.push((format!("{dir}/CLAUDE.md"), claude_md.to_string()));
            f.push((format!("{dir}/{OPERATOR_MCP_FILE}"), mcp_json.to_string()));
            Ok(())
        }
        async fn start_session(&self, project_id: i64) -> Result<SessionRow, IpcError> {
            self.starts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let s = lock(&self.store)?;
            s.upsert_session(
                OPERATOR_TMUX_NAME,
                OPERATOR_HOST,
                Some(project_id),
                None,
                1,
                1,
                "running",
                None,
            )
            .map_err(|e| IpcError::new(codes::E_SQLITE, e.to_string()))?;
            Ok(s.get_session(OPERATOR_TMUX_NAME, OPERATOR_HOST)
                .unwrap()
                .expect("the fake host just wrote this row"))
        }
    }

    /// Every LIVE client-token row the operator owns. A helper because both
    /// the count assertion and the "same token on both sides" assertion want
    /// it, and the name is the literal the brief pins.
    fn tokens_named_ux_agent(s: &Store) -> Vec<crate::store::ClientTokenRow> {
        s.list_client_tokens(false)
            .unwrap()
            .into_iter()
            .filter(|t| t.name == "ux-agent")
            .collect()
    }

    /// `ensure_operator` runs on every press of the button. Twice over it
    /// must leave one project, one session and one token — not three.
    #[tokio::test]
    async fn ensure_operator_is_idempotent() {
        let (store, _ssh, _reg) = fixture();
        let host = FakeHost::new(&store);
        let first = ensure_operator_on(&store, &host).await.expect("first");
        let second = ensure_operator_on(&store, &host).await.expect("second");
        assert_eq!(first.id, second.id, "the same session comes back");
        assert_eq!(
            host.starts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the second call must not start a second session"
        );

        // The file on the host and the hash in the database must be the
        // SAME token. Nothing else checks this: `insert_client_token` stores
        // only a hash, so a bug that wrote the wrong secret into `.mcp.json`
        // would leave both sides individually well-formed and the agent
        // permanently unauthorised.
        let files = host.files.lock().unwrap();
        let (path, body) = files
            .iter()
            .find(|(p, _)| p.ends_with(OPERATOR_MCP_FILE))
            .expect("the operator's MCP config is written");
        assert_eq!(path, &format!("{}/{OPERATOR_MCP_FILE}", host.dir));
        let v: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
        let auth = v["mcpServers"]["claude-fleet"]["headers"]["Authorization"]
            .as_str()
            .expect("a bearer header");
        let written = auth
            .strip_prefix("Bearer ")
            .expect("the header is a bearer token");
        assert_eq!(
            crate::mcp::auth::sha256_hex(written),
            store
                .lock()
                .unwrap()
                .get_setting(SETTING_OPERATOR_TOKEN_SHA)
                .unwrap()
                .expect("the operator token hash is recorded"),
            "the token handed to the host must be the one the database vouches for"
        );
        // It is also the token the client-token row carries, so revoking
        // `ux-agent` really does disarm the file on the host.
        assert_eq!(
            crate::mcp::auth::sha256_hex(written),
            tokens_named_ux_agent(&store.lock().unwrap())[0].token_sha256,
        );
        drop(files);

        let s = store.lock().unwrap();
        let projects: Vec<_> = s
            .list_projects()
            .unwrap()
            .into_iter()
            .filter(|p| p.system)
            .collect();
        assert_eq!(projects.len(), 1, "one operator project, not two");
        assert_eq!(
            tokens_named_ux_agent(&s).len(),
            1,
            "one operator token, not two"
        );
        assert_eq!(
            operator_ref(&s),
            Some(OperatorRef {
                host_alias: "local".into(),
                tmux_name: OPERATOR_TMUX_NAME.into(),
            })
        );
    }

    /// The ordering guard for `configured_port`. A fleet whose control API
    /// has never been enabled must be refused BEFORE anything is written:
    /// with the port lookup below the token work, every press revoked the
    /// live token, inserted a replacement that could never be delivered and
    /// recorded its hash, and only then returned the refusal.
    #[tokio::test]
    async fn a_fleet_with_no_control_api_is_refused_before_anything_is_mutated() {
        let (store, _ssh, _reg) = fixture();
        // A token the operator already holds, to prove it survives.
        {
            let s = lock(&store).unwrap();
            s.set_setting(crate::mcp::SETTING_TOKEN, "").unwrap();
            s.insert_client_token(OPERATOR_CLIENT_NAME, "the-old-sha", "full")
                .unwrap();
        }
        let host = FakeHost::new(&store);

        let err = ensure_operator_on(&store, &host)
            .await
            .expect_err("no control API, no operator");
        assert_eq!(err.code, codes::E_PROVISION);

        let s = lock(&store).unwrap();
        assert_eq!(
            tokens_named_ux_agent(&s)[0].token_sha256,
            "the-old-sha",
            "the live token must not be revoked by a press that cannot succeed"
        );
        assert_eq!(tokens_named_ux_agent(&s).len(), 1);
        assert!(
            s.get_setting(SETTING_OPERATOR_TOKEN_SHA).unwrap().is_none(),
            "no hash is recorded for a token that was never minted"
        );
        assert!(
            s.list_projects().unwrap().iter().all(|p| !p.system),
            "no operator project row either"
        );
        assert_eq!(host.starts.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(host.files.lock().unwrap().is_empty());
    }

    // ------------------------------------------------------- operator_status

    #[test]
    fn status_names_why_the_agent_cannot_work() {
        let (store, _ssh, _reg) = fixture();

        // The control API has to be turned ON for "absent" to be the
        // never-born answer: `operator_status` checks `enabled` FIRST (see
        // its doc comment), so an API that has never been enabled reports
        // "no_mcp" regardless of whether a reference exists. `fixture()`
        // only sets a master token, not `mcp.enabled` — this is that
        // missing setup, not a workaround for the function.
        {
            let s = store.lock().unwrap();
            s.set_setting(crate::mcp::SETTING_ENABLED, "true").unwrap();
        }

        // Never born.
        let st = operator_status(&store).unwrap();
        assert!(!st.ready);
        assert_eq!(st.blocked.as_deref(), Some("absent"));
        assert!(st.session.is_none());

        // Born, but the control API is off: an agent with no tools is a
        // chatbot, and the panel must say so rather than let it apologise.
        // This wins even though a reference is already on record, because
        // the enabled check runs before the reference is even looked up.
        {
            let s = store.lock().unwrap();
            s.set_setting(crate::mcp::SETTING_ENABLED, "false").unwrap();
            set_operator_ref(
                &s,
                &OperatorRef {
                    host_alias: "local".into(),
                    tmux_name: OPERATOR_TMUX_NAME.into(),
                },
            )
            .unwrap();
        }
        assert_eq!(
            operator_status(&store).unwrap().blocked.as_deref(),
            Some("no_mcp")
        );

        // Re-enable and give the reference a real session row, lost the way
        // a host reboot or a killed pane leaves it — `lost_at` set, status
        // `ghost`.
        let session_id = {
            let s = store.lock().unwrap();
            s.set_setting(crate::mcp::SETTING_ENABLED, "true").unwrap();
            let id = s
                .upsert_session(
                    OPERATOR_TMUX_NAME,
                    OPERATOR_HOST,
                    None,
                    None,
                    1,
                    1,
                    "running",
                    None,
                )
                .unwrap();
            // `probe_started_at: 0` disables the staleness guard so a
            // freshly-inserted row (no `last_reconciled_at` yet) is still
            // eligible — see `store::reconcile::ghost_cutoff`.
            s.mark_host_sessions_lost(OPERATOR_HOST, "test_lost", &[], 100, 0)
                .unwrap();
            id
        };
        let st = operator_status(&store).unwrap();
        assert!(!st.ready);
        assert_eq!(st.blocked.as_deref(), Some("lost"));
        assert!(
            st.session.as_ref().is_some_and(|s| s.lost_at.is_some()),
            "the lost session's own row comes back, so the panel can show when"
        );

        // Revive it (the recreate flow's own `restore_session`, which is
        // what a real recreate does) and mint a token, but revoke it
        // straight away — a deliberate act that must not re-mint itself.
        {
            let s = store.lock().unwrap();
            s.restore_session(session_id).unwrap();
            let sha = "deadbeef-token-sha";
            s.insert_client_token(OPERATOR_CLIENT_NAME, sha, "full")
                .unwrap();
            s.set_setting(SETTING_OPERATOR_TOKEN_SHA, sha).unwrap();
            s.revoke_client_token(OPERATOR_CLIENT_NAME).unwrap();
        }
        let st = operator_status(&store).unwrap();
        assert!(!st.ready);
        assert_eq!(st.blocked.as_deref(), Some("token_revoked"));
        assert!(st.session.is_some(), "the session itself is fine");

        // Mint a live one under the same name and it is finally ready.
        {
            let s = store.lock().unwrap();
            let sha = "a-live-token-sha";
            s.insert_client_token(OPERATOR_CLIENT_NAME, sha, "full")
                .unwrap();
            s.set_setting(SETTING_OPERATOR_TOKEN_SHA, sha).unwrap();
        }
        let st = operator_status(&store).unwrap();
        assert!(st.ready);
        assert!(st.blocked.is_none());
        assert!(st.session.is_some());
    }

    /// A failing `write_files` must not leave the operator's token
    /// committed. This is `provision::resolve_host_token` /
    /// `commit_host_token`'s rule — "a failed provision never strands a host
    /// on a token it never received" — applied to the one other place in the
    /// codebase that hands a host a secret: mint, deliver, THEN commit.
    #[tokio::test]
    async fn a_token_the_host_never_received_is_never_committed() {
        let (store, _ssh, _reg) = fixture();
        // A token the operator already holds and is working with.
        {
            let s = lock(&store).unwrap();
            s.insert_client_token(OPERATOR_CLIENT_NAME, "the-live-sha", "full")
                .unwrap();
            s.set_setting(SETTING_OPERATOR_TOKEN_SHA, "the-live-sha")
                .unwrap();
        }
        let host = FailingWriteHost {
            inner: FakeHost::new(&store),
        };
        let err = ensure_operator_on(&store, &host)
            .await
            .expect_err("the host refused the files");
        assert_eq!(err.code, codes::E_PROVISION);

        let s = lock(&store).unwrap();
        assert_eq!(
            tokens_named_ux_agent(&s).len(),
            1,
            "the replacement was never inserted"
        );
        assert_eq!(
            tokens_named_ux_agent(&s)[0].token_sha256,
            "the-live-sha",
            "the token that IS on a host must survive a birth that never delivered its replacement"
        );
        assert_eq!(
            s.get_setting(SETTING_OPERATOR_TOKEN_SHA)
                .unwrap()
                .as_deref(),
            Some("the-live-sha"),
            "and the database still vouches for it"
        );
        assert_eq!(
            host.inner.starts.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    /// Two presses of the button at the same instant.
    ///
    /// Before the birth lock, both passed guard #1 (no reference recorded
    /// yet), both minted a token and each revoked the other's — and because
    /// the store block and the `.mcp.json` write are separately ordered, the
    /// host could end up holding a token the database had already revoked.
    /// Every MCP call would then 401 while `operator_status` still answered
    /// `ready`, and guard #1 would short-circuit every later press, so
    /// nothing re-minted. The assertions below are the three halves of that:
    /// one session, one token, and the token on the host is the one the
    /// database vouches for.
    #[tokio::test]
    async fn two_presses_at_once_mint_one_operator() {
        let (store, _ssh, _reg) = fixture();
        // A host that yields at every await point. `FakeHost`'s own futures
        // are ready on the first poll, so `join!` would run one press to
        // completion before starting the other and the race could not
        // happen at all — the interleaving this test is about only exists
        // because the real host side talks to a disk and a tmux server.
        let host = YieldingHost {
            inner: FakeHost::new(&store),
        };
        let (a, b) = tokio::join!(
            ensure_operator_on(&store, &host),
            ensure_operator_on(&store, &host)
        );
        let a = a.expect("first press");
        let b = b.expect("second press");
        assert_eq!(a.id, b.id, "both presses name the same session");
        assert_eq!(
            host.inner.starts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a second overlapping press must not start a second session"
        );

        let files = host.inner.files.lock().unwrap();
        let written: Vec<&String> = files
            .iter()
            .filter(|(p, _)| p.ends_with(OPERATOR_MCP_FILE))
            .map(|(_, body)| body)
            .collect();
        assert_eq!(written.len(), 1, "one `.mcp.json` write, not two");
        let v: serde_json::Value = serde_json::from_str(written[0]).unwrap();
        let token = v["mcpServers"]["claude-fleet"]["headers"]["Authorization"]
            .as_str()
            .unwrap()
            .strip_prefix("Bearer ")
            .unwrap()
            .to_string();
        drop(files);

        let s = lock(&store).unwrap();
        let live = tokens_named_ux_agent(&s);
        assert_eq!(live.len(), 1, "one operator token, not two");
        assert_eq!(
            crate::mcp::auth::sha256_hex(&token),
            live[0].token_sha256,
            "the token on the host must be the live one, or every call 401s forever"
        );
        assert_eq!(
            s.get_setting(SETTING_OPERATOR_TOKEN_SHA)
                .unwrap()
                .as_deref(),
            Some(crate::mcp::auth::sha256_hex(&token).as_str()),
        );
    }

    /// [`FakeHost`] that yields to the runtime at every await point, so two
    /// concurrent `ensure_operator_on` calls really do interleave.
    struct YieldingHost {
        inner: FakeHost,
    }

    #[async_trait::async_trait]
    impl OperatorHost for YieldingHost {
        async fn resolve_dir(&self) -> Result<String, IpcError> {
            tokio::task::yield_now().await;
            self.inner.resolve_dir().await
        }
        async fn write_files(&self, d: &str, c: &str, m: &str) -> Result<(), IpcError> {
            tokio::task::yield_now().await;
            self.inner.write_files(d, c, m).await
        }
        async fn start_session(&self, project_id: i64) -> Result<SessionRow, IpcError> {
            tokio::task::yield_now().await;
            self.inner.start_session(project_id).await
        }
    }

    /// [`FakeHost`] whose `write_files` fails the way a host with no disk
    /// space or no SSH does.
    struct FailingWriteHost {
        inner: FakeHost,
    }

    #[async_trait::async_trait]
    impl OperatorHost for FailingWriteHost {
        async fn resolve_dir(&self) -> Result<String, IpcError> {
            self.inner.resolve_dir().await
        }
        async fn write_files(&self, _d: &str, _c: &str, _m: &str) -> Result<(), IpcError> {
            Err(IpcError::new(codes::E_PROVISION, "no space left on device"))
        }
        async fn start_session(&self, project_id: i64) -> Result<SessionRow, IpcError> {
            self.inner.start_session(project_id).await
        }
    }

    // --------------------------------------------- the guard's call sites

    /// One place that must call [`refuse_if_operator`], or the operator can
    /// be ended by the conversation asking for it.
    struct GuardSite {
        /// Named in the failure message.
        what: &'static str,
        source: &'static str,
        signature: &'static str,
        /// The literal the guard is called with, which is also what the
        /// refusal message names.
        label: &'static str,
    }

    /// The source of the top-level item starting with `signature`, ending at
    /// its closing brace. Only a top-level item's closing brace sits at
    /// column 0 once rustfmt has run, so the slice cannot run on into the
    /// next item. (The twin of `lifecycle_tests::item_source`; duplicated
    /// rather than shared because these are two private test modules in
    /// different files and the function is eight lines.)
    fn item_source<'a>(source: &'a str, signature: &str) -> &'a str {
        let start = source.find(signature).unwrap_or_else(|| {
            panic!("`{signature}` is no longer in its file — update this test to follow it")
        });
        let rest = &source[start..];
        let end = rest
            .find("\n}\n")
            .unwrap_or_else(|| panic!("`{signature}` has no closing brace at column 0"));
        &rest[..end + 3]
    }

    /// Every session-addressed operation the design named, pinned in the
    /// source of the function that performs it.
    ///
    /// Behavioural tests exist below for the two entry points that can be
    /// driven without a tmux server; the rest reach a host before they can
    /// be observed (and macOS CI has no tmux at all), so their call is
    /// pinned here instead, in the style of `lifecycle_tests`'
    /// `every_path_that_creates_a_tmux_session_forgets_the_kill_first`.
    ///
    /// This test exists because deleting the guard from any of these sites
    /// once left the whole workspace green — which is exactly how
    /// `restart_session` was found to have no guard at all.
    #[test]
    fn every_session_addressed_operation_asks_the_guard_first() {
        const LIFECYCLE: &str = include_str!("sessions/lifecycle.rs");
        const SAFE_KILL: &str = include_str!("safe_kill.rs");
        const MOVE: &str = include_str!("move_session/mod.rs");
        let sites = [
            GuardSite {
                what: "kill_session_with",
                source: LIFECYCLE,
                signature: "pub(super) async fn kill_session_with(",
                label: "kill_session",
            },
            GuardSite {
                what: "rename_session",
                source: LIFECYCLE,
                signature: "pub async fn rename_session(",
                label: "rename_session",
            },
            GuardSite {
                what: "recreate_session",
                source: LIFECYCLE,
                signature: "pub async fn recreate_session(",
                label: "recreate_session",
            },
            GuardSite {
                what: "safe_kill_session",
                source: SAFE_KILL,
                signature: "pub async fn safe_kill_session(",
                label: "safe_kill_session",
            },
            GuardSite {
                // The guard moved into `gather()` when the move's opening
                // sequence was extracted so a dry run (`preview()`, Task 3 of
                // the transfer-preflight project) could run exactly the
                // move's own checks — `gather()` is now the one place that
                // calls the guard, and `move_session_inner` calls `gather()`
                // (pinned by the assertion just below this loop). The guard
                // still runs before any step, on every real move; only the
                // function that holds the call moved.
                what: "gather",
                source: MOVE,
                signature: "async fn gather(",
                label: "move_session",
            },
        ];
        for site in &sites {
            let body = item_source(site.source, site.signature);
            assert!(
                body.contains("refuse_if_operator("),
                "{} no longer calls refuse_if_operator — the agent can be ended by the \
                 conversation that asked for it",
                site.what
            );
            assert!(
                body.contains(&format!("\"{}\"", site.label)),
                "{}'s guard must name `{}`, so the refusal says what was attempted",
                site.what,
                site.label
            );
        }
        // The guard living inside `gather()` only protects a real move if
        // `move_session_inner` actually calls `gather()` — pin the chain end
        // to end, so the guard cannot be lost by `move_session_inner` quietly
        // ceasing to call it (e.g. inlining its own copy of the opening
        // sequence again).
        let move_session_inner_body = item_source(MOVE, "async fn move_session_inner(");
        assert!(
            move_session_inner_body.contains("gather("),
            "move_session_inner no longer calls gather() — the move would run \
             without the operator guard, since that is the only place gather() \
             (and the guard inside it) is reached from"
        );
    }

    /// `restart_session` is the one session-addressed operation the design
    /// named that is deliberately NOT guarded. This test is the written
    /// record the omission never had: it fails if someone "fixes" it, and
    /// the message says why they should not.
    #[test]
    fn restart_session_is_deliberately_exempt_from_the_guard() {
        const LIFECYCLE: &str = include_str!("sessions/lifecycle.rs");
        let body = item_source(LIFECYCLE, "pub async fn restart_session(");
        assert!(
            !body.contains("refuse_if_operator("),
            "restart_session must stay unguarded: it IS the panel's `lost` recovery \
             (`restartOperator()` in src/lib/operator.ts), so a guard here would make the \
             agent refuse the one button that brings it back. Restarting is also not \
             destructive — the row, the transcript and the conversation all survive. If \
             you want it guarded, give the operator path a bypass FIRST."
        );
        assert!(
            body.contains("EXEMPT"),
            "the exemption must be argued at the site, not only here"
        );
    }

    // ------------------------------------------- the guard, driven for real

    /// The operator, recorded and alive, with the control API on.
    fn store_with_a_live_operator() -> Arc<Mutex<Store>> {
        let (store, _ssh, _reg) = fixture();
        {
            let s = lock(&store).unwrap();
            s.set_setting(crate::mcp::SETTING_ENABLED, "true").unwrap();
            s.upsert_session(
                OPERATOR_TMUX_NAME,
                OPERATOR_HOST,
                None,
                None,
                1,
                1,
                "running",
                None,
            )
            .unwrap();
            set_operator_ref(
                &s,
                &OperatorRef {
                    host_alias: OPERATOR_HOST.into(),
                    tmux_name: OPERATOR_TMUX_NAME.into(),
                },
            )
            .unwrap();
        }
        store
    }

    /// `kill_session` and `safe_kill_session` aimed at the operator, through
    /// their real entry points. Neither reaches a host: the guard is the
    /// first thing after argument validation, and without it both would fall
    /// through to a store lookup (`E_NOTFOUND` / the row) rather than an
    /// `E_FORBIDDEN` — so a dropped guard fails this test loudly.
    #[tokio::test]
    async fn the_kill_paths_refuse_the_operator_through_their_real_entry_points() {
        let store = store_with_a_live_operator();
        let ssh = Arc::new(crate::ssh::SshClient::new());

        let err = crate::service::sessions::kill_session(
            crate::service::sessions::KillSessionArgs {
                host_alias: OPERATOR_HOST.to_string(),
                name: OPERATOR_TMUX_NAME.to_string(),
                force: false,
            },
            &store,
            &ssh,
        )
        .await
        .expect_err("kill_session must refuse the operator");
        assert_eq!(err.code, codes::E_FORBIDDEN, "{}", err.message);

        let err = crate::service::safe_kill::safe_kill_session(
            crate::service::safe_kill::SafeKillSessionArgs {
                host_alias: OPERATOR_HOST.to_string(),
                tmux_name: OPERATOR_TMUX_NAME.to_string(),
            },
            &store,
            &ssh,
        )
        .await
        .expect_err("safe_kill_session must refuse the operator");
        assert_eq!(err.code, codes::E_FORBIDDEN, "{}", err.message);
    }
}
