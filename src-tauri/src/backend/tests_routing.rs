//! Does every command honour the resolved backend?
//!
//! Four questions, four groups of tests:
//!
//! 1. **Remote mode calls the right tool with the right arguments.** Each
//!    routed command is driven through its `routed::` function against a fake
//!    transport. The `Store` handed in is a real one, so "the hub's answer
//!    came back" is also the proof that the local path was not taken.
//! 2. **Standalone mode still runs the local service call**, asserted per
//!    command rather than assumed — that is the "standalone behaviour must
//!    not change" constraint.
//! 3. **A local-only command refuses with `E_LOCAL_ONLY`** and names where to
//!    go instead.
//! 4. **Nothing falls through unclassified.** `every_command_has_a_verdict`
//!    reads `lib.rs`'s `generate_handler!` list and fails on any command that
//!    neither routes nor guards nor is on an explicit exception list. That is
//!    the test that matters six months from now: a command quietly left on
//!    the local path in remote mode does not fail — it SSHes into a host with
//!    this machine's keys and mutates a fleet the hub also manages.

use super::*;
use crate::backend::remote;
use crate::commands;
use fleet_core::events::NoopEventBus;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

// ── the doubles ─────────────────────────────────────────────────────────────

/// Records every request and answers with one scripted payload.
///
/// A second, smaller copy of `tests_remote.rs`'s fake: that one lives inside
/// `remote.rs`'s private test module and answers a queue of raw responses,
/// which is what *it* is testing. This one only needs "what tool, what
/// arguments".
struct Fake {
    body: String,
    seen: Mutex<Vec<String>>,
}

impl Fake {
    /// Answers every call with `payload` as the tool's JSON, SSE-framed the
    /// way `POST /mcp` does (see `tests_remote.rs` for the provenance of the
    /// framing).
    fn answering(payload: &str) -> Arc<Self> {
        Arc::new(Self {
            body: format!(
                "event: message\ndata: {}\n\n",
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "content": [{ "type": "text", "text": payload }] },
                })
            ),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// `(tool, arguments)` of the single call it was given.
    fn only_call(&self) -> (String, Value) {
        let seen = self.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "expected exactly one hub call: {seen:?}");
        let v: Value = serde_json::from_str(&seen[0]).expect("a JSON-RPC body");
        assert_eq!(v["method"], "tools/call");
        (
            v["params"]["name"].as_str().expect("a tool").to_string(),
            v["params"]["arguments"].clone(),
        )
    }

    fn was_not_called(&self) {
        let seen = self.seen.lock().unwrap();
        assert!(
            seen.is_empty(),
            "this path reached the hub and must not have: {seen:?}"
        );
    }
}

#[async_trait::async_trait]
impl remote::HubTransport for Fake {
    async fn post_json(
        &self,
        _url: &str,
        _bearer: &str,
        body: String,
    ) -> Result<remote::HubResponse, String> {
        self.seen.lock().unwrap().push(body);
        Ok(remote::HubResponse {
            status: 200,
            body: self.body.clone(),
        })
    }
}

fn cfg() -> RemoteConfig {
    RemoteConfig {
        base_url: "https://hub.example.com".into(),
        token: "cl_s3cret-token".into(),
        client_name: "laptop".into(),
    }
}

fn remote_backend(fake: &Arc<Fake>) -> FleetBackend {
    FleetBackend::remote_over(cfg(), fake.clone())
}

/// A real on-disk store; `Store`'s in-memory constructor is fleet-core-test
/// only.
fn store() -> (tempfile::TempDir, Mutex<Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
    (dir, Mutex::new(store))
}

fn ssh() -> Arc<SshClient> {
    Arc::new(SshClient::new())
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

/// A minimal but complete `SessionRow`, null-stripped the way the hub leaves
/// one. Nothing in the local store ever looks like this.
const SESSION_PAYLOAD: &str = r#"{"id":42,"tmux_name":"from-the-hub","host_alias":"hetzner","created_at":1,"last_activity_at":2,"status":"running","kind":"tmux","turn_seq":0,"tags":[]}"#;
const HOST_PAYLOAD: &str = r#"{"alias":"trn","reachable":true,"hidden":false,"provisioned":true}"#;
const TASK_PAYLOAD: &str = r#"{"id":11,"state":"cancelled","created_at":1}"#;
/// A complete `MoveReport`: all twelve fields are required on the wire, the
/// last of them a whole `SessionRow` (the same one as [`SESSION_PAYLOAD`]).
const MOVE_PAYLOAD: &str = r#"{"source_session_id":7,"target_session_id":43,"from_host":"trn","to_host":"hetzner","tmux_name":"demo","claude_session_id":"abc","branch":"main","target_cwd":"/w/demo","transcript_bytes":1024,"source_killed":true,"warnings":[],"target":{"id":43,"tmux_name":"demo","host_alias":"hetzner","created_at":1,"last_activity_at":2,"status":"running","kind":"tmux","turn_seq":0,"tags":[]}}"#;

/// One row of the tables below: what to run, the tool it must name, and the
/// arguments it must send.
///
/// The closure hands back the command's own `Result`, and `check` requires it
/// to be `Ok`. It used to be discarded (`let _ = …`), which meant a payload
/// that could not deserialise into the command's return type still passed —
/// `move_session`'s case answered `"{}"` for a twelve-field `MoveReport` and
/// was green. With the result thrown away, "the same shape the local path
/// returns" was asserted by reading, not by test.
type Case = (
    &'static str,
    Value,
    &'static str,
    Box<dyn Fn(&FleetBackend, &Mutex<Store>, &Arc<SshClient>) -> Result<(), IpcError>>,
);

fn check(cases: Vec<Case>) {
    for (tool, want_args, payload, run) in cases {
        let fake = Fake::answering(payload);
        let (_dir, st) = store();
        let got = run(&remote_backend(&fake), &st, &ssh());
        let (got_tool, got_args) = fake.only_call();
        assert_eq!(got_tool, tool, "wrong tool for {tool}");
        assert_eq!(got_args, want_args, "wrong arguments for {tool}");
        if let Err(e) = got {
            panic!(
                "{tool}: the hub's answer did not come back as the command's return \
                 type, so the frontend would get an error where the local path \
                 returns a value: {e:?}"
            );
        }
    }
}

// ── 1. remote mode calls the right tool with the right arguments ────────────

/// Every routed read, as one table: the tool it must name and the arguments
/// it must send. A table rather than eighteen near-identical tests because
/// the thing under test *is* a mapping, and a mapping reads best as one.
#[test]
fn every_routed_read_names_its_tool_and_arguments() {
    use fleet_core::service::repo::SessionIdArgs;
    use fleet_core::service::repo_read::{
        RepoCommitArgs, RepoCommitDiffArgs, RepoFileArgs, RepoLogArgs,
    };
    use fleet_core::service::sessions::RelatedSessionsArgs;
    use fleet_core::service::worktrees::ListWorktreesArgs;

    check(vec![
        (
            "list_sessions",
            json!({ "summary": false, "force": true, "include_lost": true }),
            "[]",
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::list_sessions(
                    b,
                    Some(true),
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "related_sessions",
            json!({ "session_id": 7 }),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::related_sessions(
                    b,
                    RelatedSessionsArgs { session_id: 7 },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "list_hosts",
            json!({}),
            "[]",
            Box::new(|b, s, _| block_on(commands::hosts::routed::list_hosts(b, s)).map(|_| ())),
        ),
        (
            "list_accounts",
            json!({}),
            "[]",
            Box::new(|b, s, _| block_on(commands::hosts::routed::list_accounts(b, s)).map(|_| ())),
        ),
        (
            "list_projects",
            json!({ "summary": false }),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::projects::routed::list_projects(b, s)).map(|_| ())
            }),
        ),
        (
            "refresh_projects",
            json!({}),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::projects::routed::refresh_projects(b, s)).map(|_| ())
            }),
        ),
        (
            "list_worktrees",
            json!({ "project_id": 4 }),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::worktrees::routed::list_worktrees(
                    b,
                    ListWorktreesArgs {
                        project_id: Some(4),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "list_tasks",
            json!({ "requester_session_id": 2, "state": "running", "limit": 9 }),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::tasks::routed::list_tasks(
                    b,
                    Some(2),
                    Some("running".into()),
                    Some(9),
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "session_history",
            // The clamp runs on this side, so the hub is asked for the same
            // window the local store would have returned: 10_000 -> 500.
            json!({ "session_id": 7, "limit": 500 }),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::session_history(
                    b,
                    commands::sessions::SessionHistoryArgs {
                        session_id: 7,
                        limit: Some(10_000),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "session_conversation",
            json!({ "session_id": 7, "turns": 5 }),
            r#"{"turns":[],"truncated":false}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::session_conversation(
                    b,
                    commands::sessions::SessionConversationArgs {
                        session_id: 7,
                        turns: Some(5),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "repo_log",
            json!({ "session_id": 7, "all": true, "limit": 25, "skip": 50 }),
            "[]",
            Box::new(|b, s, h| {
                block_on(commands::history::routed::repo_log(
                    b,
                    RepoLogArgs {
                        session_id: 7,
                        all: true,
                        limit: 25,
                        skip: 50,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "repo_branches",
            json!({ "session_id": 7 }),
            "[]",
            Box::new(|b, s, h| {
                block_on(commands::history::routed::repo_branches(
                    b,
                    SessionIdArgs { session_id: 7 },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "repo_commit",
            json!({ "session_id": 7, "hash": "abc123" }),
            r#"{"hash":"abc123","subject":"s","body":"","author":"a","date":"d","files":[]}"#,
            Box::new(|b, s, h| {
                block_on(commands::history::routed::repo_commit(
                    b,
                    RepoCommitArgs {
                        session_id: 7,
                        hash: "abc123".into(),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "repo_commit_diff",
            json!({ "session_id": 7, "hash": "abc123", "path": "src/lib.rs" }),
            r#"{"path":"src/lib.rs","diff":"","binary":false,"truncated":false}"#,
            Box::new(|b, s, h| {
                block_on(commands::history::routed::repo_commit_diff(
                    b,
                    RepoCommitDiffArgs {
                        session_id: 7,
                        hash: "abc123".into(),
                        path: "src/lib.rs".into(),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "repo_changes",
            json!({ "session_id": 7 }),
            "[]",
            Box::new(|b, s, h| {
                block_on(commands::files::routed::repo_changes(
                    b,
                    SessionIdArgs { session_id: 7 },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "repo_tree",
            json!({ "session_id": 7 }),
            r#"{"entries":[],"truncated":false}"#,
            Box::new(|b, s, h| {
                block_on(commands::files::routed::repo_tree(
                    b,
                    SessionIdArgs { session_id: 7 },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "repo_file",
            json!({ "session_id": 7, "path": "src/lib.rs" }),
            r#"{"path":"src/lib.rs","content":"","truncated":false,"binary":false,"is_dir":false,"size":0}"#,
            Box::new(|b, s, h| {
                block_on(commands::files::routed::repo_file(
                    b,
                    RepoFileArgs {
                        session_id: 7,
                        path: "src/lib.rs".into(),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "repo_diff",
            json!({ "session_id": 7, "path": "src/lib.rs" }),
            r#"{"path":"src/lib.rs","diff":"","binary":false,"truncated":false}"#,
            Box::new(|b, s, h| {
                block_on(commands::files::routed::repo_diff(
                    b,
                    RepoFileArgs {
                        session_id: 7,
                        path: "src/lib.rs".into(),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
    ]);
}

/// The same for every routed mutation. Kept separate from the reads because
/// the cost of a wrong argument here is not a wrong screen — it is a wrong
/// action on somebody's fleet.
#[test]
fn every_routed_mutation_names_its_tool_and_arguments() {
    use fleet_core::service::bg_sessions::NewBgSessionArgs;
    use fleet_core::service::hosts::HostAliasArgs;
    use fleet_core::service::move_session::MoveSessionArgs;
    use fleet_core::service::safe_kill::SafeKillSessionArgs;
    use fleet_core::service::sessions::{
        DismissGhostSessionArgs, KillSessionArgs, RecreateSessionArgs, RenameSessionArgs,
        RestartSessionArgs, SendPromptArgs, SetFriendlyNameArgs, SpawnReviewArgs,
    };
    use fleet_core::service::worktrees::DeleteWorktreeArgs;

    check(vec![
        (
            "send_prompt",
            json!({ "host_alias": "trn", "tmux_name": "demo", "prompt": "go", "submit": true }),
            r#"{"delivered":true}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::send_prompt(
                    b,
                    SendPromptArgs {
                        host_alias: "trn".into(),
                        tmux_name: "demo".into(),
                        prompt: "go".into(),
                        submit: true,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "kill_session",
            json!({ "host_alias": "trn", "name": "demo", "force": true }),
            "7",
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::kill_session(
                    b,
                    KillSessionArgs {
                        host_alias: "trn".into(),
                        name: "demo".into(),
                        force: true,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "safe_kill_session",
            json!({ "host_alias": "trn", "tmux_name": "demo" }),
            SESSION_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::safe_kill_session(
                    b,
                    SafeKillSessionArgs {
                        host_alias: "trn".into(),
                        tmux_name: "demo".into(),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "rename_session",
            json!({ "host_alias": "trn", "old_name": "a", "new_name": "b" }),
            SESSION_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::rename_session(
                    b,
                    RenameSessionArgs {
                        host_alias: "trn".into(),
                        old_name: "a".into(),
                        new_name: "b".into(),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            // The one place the two vocabularies differ: the command is
            // `set_session_friendly_name`, the tool is `set_friendly_name`.
            "set_friendly_name",
            json!({ "host_alias": "trn", "tmux_name": "demo", "friendly_name": "the demo" }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::set_session_friendly_name(
                    b,
                    SetFriendlyNameArgs {
                        host_alias: "trn".into(),
                        tmux_name: "demo".into(),
                        friendly_name: "the demo".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "restart_session",
            json!({ "host_alias": "trn", "name": "demo", "force": false }),
            SESSION_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::restart_session(
                    b,
                    RestartSessionArgs {
                        host_alias: "trn".into(),
                        name: "demo".into(),
                        force: false,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "spawn_review",
            json!({ "source_session_id": 7, "prompt": "review it" }),
            SESSION_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::spawn_review(
                    b,
                    SpawnReviewArgs {
                        source_session_id: 7,
                        prompt: "review it".into(),
                        call_id: None,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "recreate_session",
            json!({ "session_id": 7, "force": false }),
            SESSION_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::recreate_session(
                    b,
                    RecreateSessionArgs {
                        session_id: 7,
                        force: false,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "dismiss_ghost_session",
            json!({ "session_id": 7 }),
            r#"{"dismissed":7}"#,
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::dismiss_ghost_session(
                    b,
                    DismissGhostSessionArgs { session_id: 7 },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "new_bg_session",
            json!({ "host_alias": "trn", "name": "worker", "prompt": "go" }),
            r#"{"claude_session_id":"abc"}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::new_bg_session(
                    b,
                    NewBgSessionArgs {
                        host_alias: "trn".into(),
                        name: "worker".into(),
                        prompt: "go".into(),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "delete_worktree",
            json!({ "worktree_id": 3, "force": true }),
            "worktree deleted",
            Box::new(|b, s, h| {
                block_on(commands::worktrees::routed::delete_worktree(
                    b,
                    DeleteWorktreeArgs {
                        worktree_id: 3,
                        force: true,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "cancel_task",
            json!({ "task_id": 11 }),
            TASK_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::tasks::routed::cancel_task(b, 11, s)).map(|_| ())
            }),
        ),
        (
            "probe_host",
            json!({ "alias": "trn" }),
            HOST_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::hosts::routed::probe_host(
                    b,
                    HostAliasArgs {
                        alias: "trn".into(),
                    },
                    s,
                    h,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "move_session",
            json!({ "session_id": 7, "target_host_alias": "hetzner", "keep_source": false }),
            MOVE_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::move_session::routed::move_session(
                    b,
                    MoveSessionArgs {
                        session_id: 7,
                        target_host_alias: "hetzner".into(),
                        keep_source: false,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
    ]);
}

/// The answer the UI gets in remote mode is the hub's, deserialised
/// unchanged — not merged with, and not falling back to, the local database.
#[test]
fn a_routed_read_answers_the_hub_and_not_the_local_database() {
    let fake = Fake::answering(&format!("[{SESSION_PAYLOAD}]"));
    let (_dir, st) = store();
    // Something in the local store that must NOT come back.
    st.lock().unwrap().upsert_host("trn").unwrap();

    let rows = block_on(commands::sessions::routed::list_sessions(
        &remote_backend(&fake),
        None,
        &st,
        &ssh(),
    ))
    .expect("the hub's rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, 42);
    assert_eq!(rows[0].tmux_name, "from-the-hub");
    assert_eq!(rows[0].host_alias, "hetzner");
}

/// Audit finding A: same tool, same arguments, same type — different text on
/// screen. The hub's `list_tasks` passes every row through
/// `tasks::mark_task_result`, which puts the untrusted-content marker line in
/// front of the worker's result; that is right for an AGENT reading a tool
/// answer, and wrong for a person reading the Tasks panel, which standalone
/// shows the result exactly as the worker wrote it. Parity is about what the
/// frontend receives, not only about what was sent.
#[test]
fn a_task_result_reads_the_same_as_standalone_without_the_hubs_marker() {
    use fleet_core::mcp::guard::mark_untrusted;
    use fleet_core::service::tasks::result_origin;
    // A worker whose own text starts with a line that merely LOOKS like a
    // marker must keep it: only the hub's line is removed.
    for said in ["shipped the fix", "shipped the fix\nand a second line"] {
        let marked = mark_untrusted(said, &result_origin(11, Some(7), Some("trn")));
        let payload =
            json!([{ "id": 11, "state": "done", "created_at": 1, "result": marked }]).to_string();
        let fake = Fake::answering(&payload);
        let (_dir, st) = store();
        let rows = block_on(commands::tasks::routed::list_tasks(
            &remote_backend(&fake),
            None,
            None,
            None,
            &st,
        ))
        .expect("the hub's tasks");
        assert_eq!(
            rows[0].result.as_deref(),
            Some(said),
            "standalone shows the worker's words; paired at a hub the same task \
             must not read differently"
        );
    }
}

/// `force` is the sidebar's Refresh button. Standalone it reconciles here;
/// pointed at a hub it must make the HUB reconcile, not this app — a desktop
/// reconciling a fleet it does not own is the double-brain failure.
#[test]
fn refresh_asks_whoever_owns_the_fleet_to_reconcile() {
    for (force, want) in [(Some(true), true), (Some(false), false), (None, false)] {
        let fake = Fake::answering("[]");
        let (_dir, st) = store();
        let _ = block_on(commands::sessions::routed::list_sessions(
            &remote_backend(&fake),
            force,
            &st,
            &ssh(),
        ));
        let (_, args) = fake.only_call();
        assert_eq!(args["force"], json!(want), "for force={force:?}");
    }
}

/// The Task 3 deferral, closed. `health_check` used to be exempt because it
/// returned a bare `Health`; in remote mode it therefore read the local
/// database, which a hub client never fills, and answered a **zeroed fleet**
/// — no stuck sessions, no ghosts, nothing in the red. That is the most
/// reassuring thing this app can say and it was saying it about a fleet it
/// was not looking at.
///
/// Two halves, both asserted: the hub is asked, and the hub's answer is what
/// comes back even with rows sitting in the local store.
#[test]
fn health_is_the_hubs_fleet_not_this_apps_empty_database() {
    const HEALTH_PAYLOAD: &str = r#"{"version":"9.9.9","db_ready":true,"schema_version":41,
        "hosts_reachable":3,"hosts_total":4,"sessions_total":12,"by_status":{"working":5},
        "ghosts":2,"context_red":1,"stuck":3,"usage_by_host":{},"usage_by_day":[]}"#;
    let fake = Fake::answering(HEALTH_PAYLOAD);
    let (_dir, st) = store();
    // A local row that must not be counted: the local store is not this
    // window's fleet.
    st.lock().unwrap().upsert_host("trn").unwrap();

    let got = block_on(commands::health::routed::health_check(
        &remote_backend(&fake),
        &st,
    ))
    .expect("the hub's health");

    let (tool, args) = fake.only_call();
    assert_eq!(tool, "fleet_health");
    assert_eq!(args, json!({}));
    assert_eq!(got.stuck, 3, "a zero here is the bug this test exists for");
    assert_eq!(got.ghosts, 2);
    assert_eq!(got.sessions_total, 12);
    assert_eq!(
        got.hosts_total, 4,
        "1 would mean the local store answered: it holds exactly one host"
    );
    assert_eq!(got.version, "9.9.9");
}

/// And when the hub cannot be reached, the footer must be able to say so.
/// Before this it could not: a bare `Health` had nowhere to put a failure, so
/// the only thing available to show was a zeroed one.
#[test]
fn an_unreachable_hub_makes_health_an_error_rather_than_a_zeroed_fleet() {
    struct Dead;
    #[async_trait::async_trait]
    impl remote::HubTransport for Dead {
        async fn post_json(
            &self,
            _url: &str,
            _bearer: &str,
            _body: String,
        ) -> Result<remote::HubResponse, String> {
            Err("connect hub.example.com:443: connection refused".into())
        }
    }
    let (_dir, st) = store();
    let backend = FleetBackend::remote_over(cfg(), Arc::new(Dead));
    // `Health` has no `Debug` (deliberately: see service::health), so
    // `expect_err` is not available — match instead.
    let e = match block_on(commands::health::routed::health_check(&backend, &st)) {
        Err(e) => e,
        Ok(_) => panic!("a dead hub must be an error, not a healthy-looking fleet"),
    };
    assert_eq!(e.code, codes::E_HUB_UNREACHABLE);
    assert!(!format!("{e:?}").contains("cl_s3cret-token"), "{e:?}");
}

// ── 2. standalone mode still runs the local service call ────────────────────

/// The store-backed reads answer from the store. This is the "standalone
/// behaviour must not change" constraint, asserted per command.
#[test]
fn standalone_reads_still_come_from_the_local_store() {
    let (_dir, st) = store();
    st.lock().unwrap().upsert_host("trn").unwrap();

    let local = FleetBackend::local();
    assert!(!local.is_remote());

    let hosts = block_on(commands::hosts::routed::list_hosts(&local, &st)).expect("hosts");
    assert_eq!(hosts.len(), 1, "the seeded host must come back");
    assert_eq!(hosts[0].alias, "trn");

    let accounts = block_on(commands::hosts::routed::list_accounts(&local, &st)).expect("accounts");
    assert!(accounts.is_empty());

    // Standalone health is still this process's: its own version, its own
    // database, and the fleet it owns.
    let health = block_on(commands::health::routed::health_check(&local, &st)).expect("health");
    assert_eq!(
        health.hosts_total, 1,
        "the seeded host must be counted here"
    );
    assert_eq!(health.version, fleet_core::app_version::get());

    let projects =
        block_on(commands::projects::routed::list_projects(&local, &st)).expect("projects");
    assert!(projects.is_empty());

    let tasks = block_on(commands::tasks::routed::list_tasks(
        &local, None, None, None, &st,
    ))
    .expect("tasks");
    assert!(tasks.is_empty());

    let events = block_on(commands::sessions::routed::session_history(
        &local,
        commands::sessions::SessionHistoryArgs {
            session_id: 1,
            limit: None,
        },
        &st,
    ))
    .expect("history");
    assert!(events.is_empty());
}

/// The SSH-backed reads take the local path too. Proof without a network:
/// the local path resolves the session id against the store first and answers
/// `E_NOTFOUND` for an unknown one — an error only it can produce, since the
/// hub arm would have returned the fake's payload instead.
#[test]
fn standalone_ssh_backed_reads_take_the_local_path() {
    use fleet_core::service::repo::SessionIdArgs;
    let (_dir, st) = store();
    let local = FleetBackend::local();

    for (what, err) in [
        (
            "repo_changes",
            block_on(commands::files::routed::repo_changes(
                &local,
                SessionIdArgs { session_id: 999 },
                &st,
                &ssh(),
            ))
            .err(),
        ),
        (
            "repo_branches",
            block_on(commands::history::routed::repo_branches(
                &local,
                SessionIdArgs { session_id: 999 },
                &st,
                &ssh(),
            ))
            .err(),
        ),
        (
            "session_conversation",
            block_on(commands::sessions::routed::session_conversation(
                &local,
                commands::sessions::SessionConversationArgs {
                    session_id: 999,
                    turns: None,
                },
                &st,
                &ssh(),
            ))
            .err(),
        ),
    ] {
        let err = err.unwrap_or_else(|| panic!("{what}: an unknown session must not reach a host"));
        assert_eq!(err.code, codes::E_NOTFOUND, "for {what}: {}", err.message);
    }
}

// ── 3. the local-only refusals ──────────────────────────────────────────────

#[test]
fn a_local_only_command_is_a_no_op_when_standalone() {
    assert!(FleetBackend::local()
        .local_only("catalog_push", "do it there")
        .is_ok());
}

#[test]
fn a_local_only_command_refuses_in_remote_mode_and_says_where_to_go() {
    let fake = Fake::answering("[]");
    let err = remote_backend(&fake)
        .local_only("provision_hosts", "provision from the hub with `fleet-hub`")
        .expect_err("a local-only command must refuse");
    assert_eq!(err.code, codes::E_LOCAL_ONLY);
    assert!(err.message.contains("provision_hosts"), "{}", err.message);
    assert!(
        err.message.contains("provision from the hub"),
        "the message must name what to do instead: {}",
        err.message
    );
    assert!(
        err.message.contains("hub.example.com"),
        "and which hub is in the way: {}",
        err.message
    );
    fake.was_not_called();
}

/// The refusal message carries the hub's URL, so it is an outward string and
/// gets the same scrutiny as every other one in this module.
#[test]
fn a_refusal_never_carries_the_token() {
    let fake = Fake::answering("[]");
    let err = remote_backend(&fake)
        .local_only("catalog_push", "push from the hub")
        .unwrap_err();
    assert!(!format!("{err:?}").contains("cl_s3cret-token"), "{err:?}");
    assert!(!format!("{:?}", remote_backend(&fake)).contains("cl_s3cret-token"));
}

/// A refusal's reason is what the user acts on, so it has to be TRUE —
/// `local_only`'s own doc says it "must tell the user where the operation
/// does work". The audit found seven that were not:
///
/// - six asset commands said "the hub exposes no authoring tool" while the
///   hub has a tool for each (`list_assets` is even read-only, served to any
///   paired phone), and two of those are master-only, which is the real
///   reason they refuse;
/// - `dismiss_agent_session` said "the hub exposes no tool for it" while the
///   routed `kill_session` dismisses an inactive agent exactly as it does.
///
/// Read from source, like `every_command_has_a_verdict`, because a
/// `#[tauri::command]` cannot be called without a live `tauri::App`.
#[test]
fn a_refusal_that_has_a_hub_tool_names_it_rather_than_denying_it() {
    fn reason(file: &str, name: &str) -> String {
        let src = SOURCES
            .iter()
            .find(|(f, _)| *f == file)
            .unwrap_or_else(|| panic!("{file} is not in SOURCES"))
            .1;
        let start = src
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("no fn {name} in {file}"));
        let rest = &src[start..];
        let body = &rest[..rest.find("#[tauri::command]").unwrap_or(rest.len())];
        let call = &body[body.find("local_only(").expect("a local_only guard")..];
        let call = &call[..call.find(")?").expect("the guard's end")];
        call.replace("\\\n", " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
    const DENIALS: [&str; 2] = ["exposes no authoring tool", "exposes no tool"];

    for (command, tool) in [
        ("catalog_list_assets", "list_assets"),
        ("assets_scan_hosts", "scan_assets"),
        ("catalog_import_host", "import_assets"),
        ("catalog_plan_sync", "plan_sync"),
        ("catalog_apply_sync", "apply_sync"),
        ("catalog_set_secret", "set_secret"),
    ] {
        let said = reason("commands/assets.rs", command);
        for d in DENIALS {
            assert!(
                !said.contains(d),
                "{command} denies a tool the hub has ({tool}): {said}"
            );
        }
        assert!(
            said.contains(tool),
            "{command} must name the hub's {tool}: {said}"
        );
    }
    // The two the hub keeps for its master: THAT is why they refuse.
    for command in ["catalog_apply_sync", "catalog_set_secret"] {
        let said = reason("commands/assets.rs", command);
        assert!(said.contains("master"), "{command}: {said}");
    }

    let said = reason("commands/sessions.rs", "dismiss_agent_session");
    for d in DENIALS {
        assert!(!said.contains(d), "dismiss_agent_session: {said}");
    }
    assert!(
        said.contains("kill_session"),
        "the routed Kill does this for an inactive agent, and the user should \
         be sent there: {said}"
    );
}

// ── 4. nothing falls through unclassified ───────────────────────────────────

/// Commands that are deliberately the same in both modes, each with its
/// reason. Anything not routed, not guarded and not on this list fails
/// [`every_command_has_a_verdict`].
const SAME_IN_BOTH_MODES: &[(&str, &str)] = &[
    (
        "collect_diagnostics",
        "describes THIS process — its log tail, its tunnels, its SSH counters \
         — and is the first thing asked for when remote mode misbehaves",
    ),
    (
        "open_log_folder",
        "this app's own log folder, which it has either way",
    ),
    (
        "cancel_command",
        "the cancellation registry is this process's, and the call it cancels \
         is one this process started",
    ),
    (
        "mcp_confirm",
        "answers this process's own confirm queue, which is empty in remote \
         mode — answering nothing is correct",
    ),
    ("mcp_pending_confirms", "the same queue, the same reason"),
    (
        "hub_status",
        "reports which fleet THIS window is onto. Asking a hub would be \
         circular, and Settings needs the answer most when the hub is \
         unreachable",
    ),
    (
        "hub_pair",
        "points this process at a hub. It talks to POST /pair — the one \
         unauthenticated route, and not an MCP tool at all",
    ),
    (
        "hub_disconnect",
        "forgets this machine's own token and setting. It revokes nothing on \
         the hub: only an operator can, and a paired client is refused \
         revoke_client by design",
    ),
    (
        "pty_write",
        "acts on whatever is attached; with pty_open refused nothing ever is, \
         so E_PTY_CLOSED is the true answer",
    ),
    ("pty_resize", "the same as pty_write"),
    ("pty_drain", "the same as pty_write"),
    (
        "pty_close",
        "the same as pty_write — guarding it would make closing fail",
    ),
];

/// **The test that matters six months from now.**
///
/// Reads the `generate_handler!` list out of `lib.rs` and checks that every
/// command in it has a verdict: it either routes on the backend, refuses with
/// `local_only`, or is named in [`SAME_IN_BOTH_MODES`] with a reason.
///
/// Source-scanning is a blunt instrument, and this is the one place it earns
/// its keep. What it guards against is not a wrong answer but a *missing
/// decision*: a command added later, wired into the handler list, and left
/// running the local service path — which in remote mode means SSHing into
/// hosts with this machine's keys and mutating a fleet the hub also manages.
#[test]
fn every_command_has_a_verdict() {
    let lib = include_str!("../lib.rs");
    let handlers = lib
        .split_once("generate_handler![")
        .expect("lib.rs must still register its commands with generate_handler!")
        .1
        .split_once("])")
        .expect("an unterminated generate_handler! list")
        .0;

    // `commands::sessions::list_sessions` -> ("commands/sessions.rs", "list_sessions")
    // `pty::pty_open`                     -> ("pty.rs", "pty_open")
    // `cancel_command`                    -> ("lib.rs", "cancel_command")
    let mut entries: Vec<(String, String)> = Vec::new();
    for raw in handlers.split(',') {
        let path = raw.trim();
        if path.is_empty() {
            continue;
        }
        let parts: Vec<&str> = path.split("::").collect();
        let name = (*parts.last().unwrap()).to_string();
        let file = match parts.as_slice() {
            ["commands", module, _] => format!("commands/{module}.rs"),
            ["pty", _] => "pty.rs".to_string(),
            [_] => "lib.rs".to_string(),
            other => panic!("unexpected handler entry {other:?}"),
        };
        entries.push((file, name));
    }
    assert!(
        entries.len() > 90,
        "only {} handlers parsed — the parser broke, not the code",
        entries.len()
    );

    let sources: std::collections::BTreeMap<&str, &str> = SOURCES.iter().copied().collect();
    let mut unclassified = Vec::new();
    for (file, name) in &entries {
        if SAME_IN_BOTH_MODES.iter().any(|(n, _)| n == name) {
            continue;
        }
        let src = sources
            .get(file.as_str())
            .unwrap_or_else(|| panic!("add {file} to SOURCES in tests_routing.rs"));
        let start = src
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("{name} is registered but not defined in {file}"));
        // This command's body, up to wherever the next one begins.
        let rest = &src[start..];
        let body = match rest.find("#[tauri::command]") {
            Some(i) => &rest[..i],
            None => rest,
        };
        if !body.contains("routed::") && !body.contains("local_only(") {
            unclassified.push(format!("{file}::{name}"));
        }
    }

    assert!(
        unclassified.is_empty(),
        "these commands neither route on the backend nor refuse with \
         E_LOCAL_ONLY, so in remote mode they silently run against THIS \
         machine's database and SSH keys, on a fleet the hub also \
         manages:\n  {}\n\nGive each one a verdict: route it through a \
         `routed::` function, guard it with `backend.local_only(...)`, or add \
         it to SAME_IN_BOTH_MODES with the reason.",
        unclassified.join("\n  ")
    );
}

/// Every source file [`every_command_has_a_verdict`] needs to read.
const SOURCES: &[(&str, &str)] = &[
    (
        "commands/account_usage.rs",
        include_str!("../commands/account_usage.rs"),
    ),
    ("commands/assets.rs", include_str!("../commands/assets.rs")),
    (
        "commands/diagnostics.rs",
        include_str!("../commands/diagnostics.rs"),
    ),
    ("commands/files.rs", include_str!("../commands/files.rs")),
    ("commands/health.rs", include_str!("../commands/health.rs")),
    (
        "commands/history.rs",
        include_str!("../commands/history.rs"),
    ),
    ("commands/hosts.rs", include_str!("../commands/hosts.rs")),
    ("commands/hub.rs", include_str!("../commands/hub.rs")),
    ("commands/mcp.rs", include_str!("../commands/mcp.rs")),
    (
        "commands/move_session.rs",
        include_str!("../commands/move_session.rs"),
    ),
    ("commands/mutate.rs", include_str!("../commands/mutate.rs")),
    (
        "commands/onboarding.rs",
        include_str!("../commands/onboarding.rs"),
    ),
    (
        "commands/projects.rs",
        include_str!("../commands/projects.rs"),
    ),
    (
        "commands/sessions.rs",
        include_str!("../commands/sessions.rs"),
    ),
    ("commands/tasks.rs", include_str!("../commands/tasks.rs")),
    ("commands/upload.rs", include_str!("../commands/upload.rs")),
    (
        "commands/worktrees.rs",
        include_str!("../commands/worktrees.rs"),
    ),
    ("pty.rs", include_str!("../pty.rs")),
    ("lib.rs", include_str!("../lib.rs")),
];
