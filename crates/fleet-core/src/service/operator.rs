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

/// [`ensure_operator`] with the host side injected. The store guard is taken
/// in four short scoped blocks below and dropped before every `.await`:
/// this function interleaves database work with SSH and tmux work, and
/// holding the mutex across either would block every other command in the
/// app for the length of a network round trip.
pub(crate) async fn ensure_operator_on(
    store: &Arc<Mutex<Store>>,
    host: &dyn OperatorHost,
) -> Result<SessionRow, IpcError> {
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

    // 3. The project row and the token, under one guard. (#3)
    let (project_id, token) = {
        let s = lock(store)?;
        let project_id = s
            .upsert_system_project(OPERATOR_OWNER, OPERATOR_REPO, &dir)
            .map_err(|e| IpcError::new(codes::E_SQLITE, format!("operator project row: {e}")))?;
        // Only the hash is ever stored; the plaintext lives in the
        // operator's `.mcp.json` on the host and nowhere else.
        let token = crate::mcp::generate_token();
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
        (project_id, token)
    };

    // 4. The files on the host, then the session itself.
    host.write_files(&dir, claude_md(), &mcp_json(&endpoint, &token))
        .await?;
    let row = host.start_session(project_id).await?;

    // 5. Record where it lives, so the self-guard and the next press can
    //    find it. (guard #4)
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
}
