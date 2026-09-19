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
//!    reads `lib.rs`'s `generate_handler!` list and holds it to exactly the
//!    set of names in [`VERDICTS`](super::verdicts::VERDICTS), and
//!    `every_commands_body_does_what_its_row_says` holds each command's body
//!    to the row it has. That is the pair that matters six months from now: a
//!    command quietly left on the local path in remote mode does not fail —
//!    it SSHes into a host with this machine's keys and mutates a fleet the
//!    hub also manages.

use super::verdicts::VERDICTS;
use super::*;
use crate::backend::remote;
use crate::commands;
use fleet_core::events::NoopEventBus;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
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
/// `transport` is required on the wire (migration 034): it is a `String`,
/// not an `Option`, so `ok_json_compact` never strips it and a row without
/// it does not parse.
const HOST_PAYLOAD: &str =
    r#"{"alias":"trn","reachable":true,"hidden":false,"provisioned":true,"transport":"ssh"}"#;
const TASK_PAYLOAD: &str = r#"{"id":11,"state":"cancelled","created_at":1}"#;
/// A complete `MoveReport`: all twelve fields are required on the wire, the
/// last of them a whole `SessionRow` (the same one as [`SESSION_PAYLOAD`]).
const MOVE_PAYLOAD: &str = r#"{"source_session_id":7,"target_session_id":43,"from_host":"trn","to_host":"hetzner","tmux_name":"demo","claude_session_id":"abc","branch":"main","target_cwd":"/w/demo","transcript_bytes":1024,"source_killed":true,"warnings":[],"carried":{"commits":2,"bundle_bytes":1234,"dirty_entries":[{"status":" M","path":"src/lib.rs"}],"ignored_carried":[{"path":".env","bytes":4096}],"ignored_left_behind":[{"path":"node_modules/","bytes":null,"reason":"denylisted"}],"target_seeded":"existing"},"target":{"id":43,"tmux_name":"demo","host_alias":"hetzner","created_at":1,"last_activity_at":2,"status":"running","kind":"tmux","turn_seq":0,"tags":[]}}"#;
/// A complete `RepairReport` (`repair_session` uses `ok_json`, not the
/// null-stripping `ok_json_compact`, so every `Option` is present as a real
/// key — `null` included — and every non-`Option` field is required).
const REPAIR_PAYLOAD: &str = r#"{"session_id":7,"host_alias":"trn","tmux_name":"demo","project_root":"/p","cwd":"/p","cwd_physical":null,"healthy":true,"actions":[],"warnings":[],"needs_explicit_repair":false,"deferred":[],"branch_source":null,"tmux":null,"tmux_alive":true,"tmux_dead":false,"tmux_cwd_stale":false,"worktree_row_updated":false,"sibling_session_ids":[],"vanished_guard":null}"#;

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
    check(routed_read_cases());
}

/// The table [`every_routed_read_names_its_tool_and_arguments`] runs; also run against a configured hub this
/// launch cannot use, which must refuse every row.
fn routed_read_cases() -> Vec<Case> {
    use fleet_core::service::repo::SessionIdArgs;
    use fleet_core::service::repo_read::{
        RepoCommitArgs, RepoCommitDiffArgs, RepoFileArgs, RepoLogArgs,
    };
    use fleet_core::service::sessions::RelatedSessionsArgs;
    use fleet_core::service::worktrees::ListWorktreesArgs;

    vec![
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
            json!({ "session_id": 7, "turns": 5, "events_limit": 200 }),
            r#"{"turns":[],"truncated":false}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::session_conversation(
                    b,
                    commands::sessions::SessionConversationArgs {
                        session_id: 7,
                        turns: Some(5),
                        claude_session_id: None,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "session_conversation",
            json!({ "session_id": 7, "turns": 5, "claude_session_id": "11111111-1111-1111-1111-111111111111", "events_limit": 200 }),
            r#"{"turns":[],"truncated":false}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::session_conversation(
                    b,
                    commands::sessions::SessionConversationArgs {
                        session_id: 7,
                        turns: Some(5),
                        claude_session_id: Some("11111111-1111-1111-1111-111111111111".into()),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "session_conversations",
            // Clamped on this side, like session_history: 10_000 -> 500.
            json!({ "session_id": 7, "limit": 500 }),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::session_conversations(
                    b,
                    commands::sessions::SessionConversationsArgs {
                        session_id: 7,
                        limit: Some(10_000),
                    },
                    s,
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
    ]
}

/// The same for every routed mutation. Kept separate from the reads because
/// the cost of a wrong argument here is not a wrong screen — it is a wrong
/// action on somebody's fleet.
#[test]
fn every_routed_mutation_names_its_tool_and_arguments() {
    check(routed_mutation_cases());
}

/// The table [`every_routed_mutation_names_its_tool_and_arguments`] runs; also run against a configured hub this
/// launch cannot use, which must refuse every row.
fn routed_mutation_cases() -> Vec<Case> {
    use commands::sessions::RepairSessionArgs;
    use fleet_core::service::bg_sessions::NewBgSessionArgs;
    use fleet_core::service::hosts::HostAliasArgs;
    use fleet_core::service::move_session::MoveSessionArgs;
    use fleet_core::service::safe_kill::SafeKillSessionArgs;
    use fleet_core::service::sessions::{
        DismissGhostSessionArgs, KillSessionArgs, NewSessionArgs, RecreateSessionArgs,
        RenameSessionArgs, RestartSessionArgs, SendPromptArgs, SetFriendlyNameArgs,
        SpawnReviewArgs,
    };
    use fleet_core::service::worktrees::DeleteWorktreeArgs;

    vec![
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
            json!({ "session_id": 7, "target_host_alias": "hetzner", "keep_source": false, "strict": true }),
            MOVE_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::move_session::routed::move_session(
                    b,
                    MoveSessionArgs {
                        session_id: 7,
                        target_host_alias: "hetzner".into(),
                        keep_source: false,
                        strict: true,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        // Task 1 (#146): `kind`, `start_command` and `friendly_name` now map
        // one-to-one onto the tool's `NewSessionParams`, so `new_session`
        // routes unconditionally (`call_id` is this process's own
        // cancellation-registry key and has no counterpart — never sent).
        (
            "new_session",
            json!({
                "host_alias": "trn",
                "project_id": 4,
                "worktree_id": 9,
                "name": "demo",
                "new_worktree": "feat",
                "base_branch": "main",
                "kind": "shell",
                "start_command": "pnpm dev",
                "friendly_name": "the demo",
            }),
            SESSION_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::new_session(
                    b,
                    NewSessionArgs {
                        host_alias: "trn".into(),
                        project_id: 4,
                        worktree_id: Some(9),
                        name: "demo".into(),
                        call_id: Some(123),
                        new_worktree: Some("feat".into()),
                        base_branch: Some("main".into()),
                        kind: Some("shell".into()),
                        start_command: Some("pnpm dev".into()),
                        friendly_name: Some("the demo".into()),
                    },
                    s,
                    h,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        // `explicit: true` — the Repair workspace button — routes to the
        // tool's own (always-explicit) repair.
        (
            "repair_session",
            json!({ "session_id": 7 }),
            REPAIR_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::repair_session(
                    b,
                    RepairSessionArgs {
                        session_id: 7,
                        explicit: true,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
    ]
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
                    claude_session_id: None,
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

// ── 2b. a configured hub this launch cannot use ─────────────────────────────

/// What `lib.rs` builds when `hub.remote_url` is set but the hub cannot be
/// used — here, the final review's likely real trigger, a locked keychain.
fn unavailable_backend() -> FleetBackend {
    FleetBackend::from_resolved(&Backend::Unavailable(super::super::UnavailableHub {
        url: Some("https://hub.example.com".into()),
        reason: "cannot read the client token for https://hub.example.com (the keychain \
                 is locked)"
            .into(),
    }))
}

/// F1, the command half. Starting no tick is not enough: a routed command
/// that fell back to its standalone arm would still SSH into the hub's hosts
/// with this machine's keys, and `list_sessions` would run a reconcile pass of
/// its own when the cache is stale. Every routed read and every routed
/// mutation refuses instead, names the reason, and never reaches a network.
#[test]
fn a_configured_but_unavailable_hub_refuses_every_routed_command() {
    let backend = unavailable_backend();
    for (tool, _, _, run) in routed_read_cases()
        .into_iter()
        .chain(routed_mutation_cases())
    {
        let (_dir, st) = store();
        let err = run(&backend, &st, &ssh()).expect_err(&format!(
            "{tool} ran with a configured hub this launch cannot use; its local \
             arm would manage the hub's fleet from this machine"
        ));
        assert_eq!(err.code, codes::E_HUB_UNAVAILABLE, "{tool}: {err:?}");
        assert!(
            err.message.contains("the keychain is locked"),
            "{tool}: the refusal must carry the reason: {}",
            err.message
        );
        assert!(
            err.message.contains("Settings"),
            "{tool}: and where to fix it: {}",
            err.message
        );
    }
}

/// And the local-only commands, which in a working hub client say "do it on
/// the hub", say what is actually wrong instead.
#[test]
fn a_configured_but_unavailable_hub_refuses_local_only_commands_with_the_reason() {
    let err = unavailable_backend()
        .local_only("provision_hosts", "provision from the hub with `fleet-hub`")
        .expect_err("nothing may run against the hub's fleet from here");
    assert_eq!(err.code, codes::E_HUB_UNAVAILABLE);
    assert!(err.message.contains("provision_hosts"), "{}", err.message);
    assert!(
        err.message.contains("the keychain is locked"),
        "{}",
        err.message
    );
    assert!(err.message.contains("Settings"), "{}", err.message);
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

/// Task 1 (#146), the controller ruling: `repair_session` routes only
/// `explicit: true` (the Repair workspace button, which maps one-to-one onto
/// the tool's always-explicit repair). `explicit: false` — the automatic
/// pre-attach check — has no hub counterpart, and must never be silently
/// upgraded into a destructive explicit repair; it stays local-only in remote
/// mode. Standalone, both still reach the local service call unchanged
/// (proven by `E_NOTFOUND` for an unknown session, which only that path can
/// answer).
#[test]
fn repair_session_explicit_false_stays_local_only_in_remote_mode() {
    use commands::sessions::RepairSessionArgs;

    let fake = Fake::answering("[]");
    let (_dir, st) = store();
    let err = block_on(commands::sessions::routed::repair_session(
        &remote_backend(&fake),
        RepairSessionArgs {
            session_id: 7,
            explicit: false,
        },
        &st,
        &ssh(),
    ))
    .expect_err("an automatic check must never become a destructive explicit repair");
    assert_eq!(err.code, codes::E_LOCAL_ONLY, "{err:?}");
    assert!(err.message.contains("repair_session"), "{}", err.message);
    assert!(
        err.message.contains("EXPLICIT"),
        "must say why explicit and automatic are not the same operation: {}",
        err.message
    );
    fake.was_not_called();

    for explicit in [false, true] {
        let (_dir, st) = store();
        let err = block_on(commands::sessions::routed::repair_session(
            &FleetBackend::local(),
            RepairSessionArgs {
                session_id: 999,
                explicit,
            },
            &st,
            &ssh(),
        ))
        .expect_err("standalone must still reach the local service call");
        assert_eq!(
            err.code,
            codes::E_NOTFOUND,
            "explicit={explicit}: only the local path answers this: {err:?}"
        );
    }
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
/// The four layer commands merged from main are held to the same standard:
/// the hub grew a tool for each, three of them read-only, and
/// `set_host_layers` is master-only — which is the real reason THAT one
/// refuses, the same shape as `apply_sync` and `set_secret`.
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
        ("catalog_list_layers", "list_layers"),
        ("catalog_resolve_preview", "resolve_preview"),
        ("catalog_propose_layers", "propose_layers"),
        ("catalog_set_host_layers", "set_host_layers"),
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
    // The three the hub keeps for its master: THAT is why they refuse.
    for command in [
        "catalog_apply_sync",
        "catalog_set_secret",
        "catalog_set_host_layers",
    ] {
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

// ── 3b. the refusal messages, byte for byte ─────────────────────────────────

/// The recorded messages, relative to `src-tauri/` (`CARGO_MANIFEST_DIR`).
const LOCAL_ONLY_GOLDEN_PATH: &str = "src/backend/local_only.golden.json";
/// Set to rewrite the fixture. Regenerating it is never part of a refactor:
/// the whole point of the file is that a message which changed shows up as a
/// diff someone has to read.
const REGEN_LOCAL_ONLY: &str = "REGEN_LOCAL_ONLY";

/// Read the Rust string literal starting at `src[at]` (which must be the
/// opening quote), returning its **value** and the index just past the close.
///
/// The one escape that matters here is `\` at end of line: the sentences are
/// written as continued literals, and the continuation swallows the newline
/// *and* the indentation of the next line. Getting that wrong would record a
/// message with stray spaces in it, so it is spelled out rather than
/// approximated with `split_whitespace`.
fn rust_string_literal(src: &str, at: usize) -> (String, usize) {
    let b = src.as_bytes();
    assert_eq!(b[at], b'"', "not a string literal at {at}");
    let mut out = String::new();
    let mut i = at + 1;
    while i < b.len() {
        match b[i] {
            b'"' => return (out, i + 1),
            b'\\' => {
                i += 1;
                match b[i] {
                    b'\n' => {
                        i += 1;
                        while b[i].is_ascii_whitespace() {
                            i += 1;
                        }
                    }
                    b'"' => {
                        out.push('"');
                        i += 1;
                    }
                    b'\\' => {
                        out.push('\\');
                        i += 1;
                    }
                    b'n' => {
                        out.push('\n');
                        i += 1;
                    }
                    other => panic!("unhandled escape \\{} in a refusal", other as char),
                }
            }
            _ => {
                let c = src[i..].chars().next().unwrap();
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    panic!("unterminated string literal");
}

/// Every `(what, instead)` pair pasted at a `backend.local_only(…)` call site,
/// read out of the command sources.
///
/// This is how the fixture below is *generated*, and it only works while the
/// sentences are literals at the call sites. Once they come from one table the
/// generator is gone and the fixture is the record of what they said.
fn pasted_local_only_pairs() -> BTreeMap<String, String> {
    const CALL: &str = "backend.local_only(";
    let mut out = BTreeMap::new();
    for (file, src) in SOURCES {
        let mut from = 0;
        while let Some(rel) = src[from..].find(CALL) {
            let mut i = from + rel + CALL.len();
            let b = src.as_bytes();
            while b[i].is_ascii_whitespace() {
                i += 1;
            }
            let (what, next) = rust_string_literal(src, i);
            let mut i = next;
            while b[i] != b'"' {
                i += 1;
            }
            let (instead, next) = rust_string_literal(src, i);
            assert!(
                out.insert(what.clone(), instead).is_none(),
                "{file}: {what} refuses twice with different words"
            );
            from = next;
        }
    }
    out
}

/// `command -> the whole E_LOCAL_ONLY message`, for the fixed hub of [`cfg`].
fn local_only_messages(pairs: &BTreeMap<String, String>) -> String {
    let fake = Fake::answering("[]");
    let backend = remote_backend(&fake);
    let rendered: BTreeMap<&str, String> = pairs
        .iter()
        .map(|(what, instead)| {
            let err = backend
                .local_only(what, instead)
                .expect_err("a local-only command must refuse in remote mode");
            assert_eq!(err.code, codes::E_LOCAL_ONLY, "{what}: {err:?}");
            (what.as_str(), err.message)
        })
        .collect();
    fake.was_not_called();
    let mut json = serde_json::to_string_pretty(&rendered).expect("serialisable");
    json.push('\n');
    json
}

/// **The message the user reads, pinned.**
///
/// A refusal's sentence is the whole of what a hub-client desktop tells
/// someone who clicked a button that will not work here. Moving those
/// sentences out of the call sites is only safe if "the same sentence" can be
/// checked rather than eyeballed, so every one of them is recorded — rendered,
/// not as a fragment — in a committed fixture, generated from the call sites
/// before they moved. Nothing in this task may change that file.
#[test]
fn every_local_only_message_is_the_one_the_fixture_records() {
    let pairs = pasted_local_only_pairs();
    assert!(
        pairs.len() > 70,
        "only {} refusals found — the reader broke, not the code",
        pairs.len()
    );
    let actual = local_only_messages(&pairs);

    if std::env::var(REGEN_LOCAL_ONLY).is_ok() {
        let abs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(LOCAL_ONLY_GOLDEN_PATH);
        std::fs::write(abs, &actual).expect("write the fixture");
        panic!(
            "{LOCAL_ONLY_GOLDEN_PATH} was rewritten. Read `git diff -- \
             src-tauri/{LOCAL_ONLY_GOLDEN_PATH}`: every line that changed is a \
             sentence a user reads. Then unset {REGEN_LOCAL_ONLY} and run again."
        );
    }

    let golden = include_str!("local_only.golden.json");
    if golden != actual {
        let want: BTreeMap<String, String> =
            serde_json::from_str(golden).expect("the fixture must be command -> message");
        let got: BTreeMap<String, String> = serde_json::from_str(&actual).unwrap();
        let mut complaints = Vec::new();
        for (name, msg) in &want {
            match got.get(name) {
                None => complaints.push(format!("{name} no longer refuses")),
                Some(now) if now != msg => {
                    complaints.push(format!("{name}:\n  was: {msg}\n  now: {now}"))
                }
                Some(_) => {}
            }
        }
        for name in got.keys() {
            if !want.contains_key(name) {
                complaints.push(format!("{name} refuses and did not before"));
            }
        }
        panic!(
            "the E_LOCAL_ONLY messages are not the ones \
             {LOCAL_ONLY_GOLDEN_PATH} records:\n\n{}",
            complaints.join("\n")
        );
    }
}

// ── 4. nothing falls through unclassified ───────────────────────────────────

/// `lib.rs`'s `generate_handler!` list, as `(file, command)`.
///
/// `commands::sessions::list_sessions` -> ("commands/sessions.rs", "list_sessions")
/// `pty::pty_open`                     -> ("pty.rs", "pty_open")
/// `cancel_command`                    -> ("lib.rs", "cancel_command")
fn registered_commands() -> Vec<(String, String)> {
    let lib = include_str!("../lib.rs");
    let handlers = lib
        .split_once("generate_handler![")
        .expect("lib.rs must still register its commands with generate_handler!")
        .1
        .split_once("])")
        .expect("an unterminated generate_handler! list")
        .0;

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
    entries
}

/// **The test that matters six months from now.**
///
/// Reads the `generate_handler!` list out of `lib.rs` and holds it to exactly
/// the set of names in [`VERDICTS`] — no command without a verdict, no verdict
/// for a command that no longer exists, no name twice.
///
/// What it guards against is not a wrong answer but a *missing decision*: a
/// command added later, wired into the handler list, and left running the
/// local service path — which in remote mode means SSHing into hosts with
/// this machine's keys and mutating a fleet the hub also manages.
///
/// This replaces the old `SAME_IN_BOTH_MODES` exception list: "deliberately
/// the same in both modes" is now a verdict like the other two, with its
/// reason in the same row, so a deliberate decision and a forgotten one still
/// cannot look alike.
#[test]
fn every_command_has_a_verdict() {
    let registered: BTreeSet<String> = registered_commands()
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    let mut seen = BTreeSet::new();
    let mut twice = Vec::new();
    for (name, _) in VERDICTS {
        if !seen.insert((*name).to_string()) {
            twice.push(*name);
        }
    }
    assert!(
        twice.is_empty(),
        "VERDICTS names these commands more than once, so which row wins is \
         whichever comes first: {twice:?}"
    );

    let missing: Vec<&String> = registered.difference(&seen).collect();
    assert!(
        missing.is_empty(),
        "these commands are registered in generate_handler! but have no row in \
         VERDICTS, so in remote mode they silently run against THIS machine's \
         database and SSH keys, on a fleet the hub also manages:\n  {}\n\n\
         Give each one a row: Routed with the hub tool it calls, LocalOnly \
         with the sentence it refuses with, or SameInBoth with the reason.",
        missing
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    let stale: Vec<&String> = seen.difference(&registered).collect();
    assert!(
        stale.is_empty(),
        "VERDICTS has rows for commands that generate_handler! no longer \
         registers — a verdict about nothing reads like a verdict about \
         something:\n  {}",
        stale
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
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
