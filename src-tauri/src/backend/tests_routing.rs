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

use super::verdicts::{self, Verdict, VERDICTS};
use super::*;
use crate::backend::connection::{self, ConnectionReporter};
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

/// A hub client in its working state: its hub's `ready` frame has been
/// judged in range on this launch (the event bridge reported `Connected`).
/// `move_session`'s dry run is refused on any client that has not got this
/// far — see `a_dry_run_is_refused_until_this_launch_has_confirmed_the_hub`.
fn remote_backend(fake: &Arc<Fake>) -> FleetBackend {
    let link = Arc::new(connection::HubConnectionStatus::remote(
        Arc::new(Silent),
        &cfg().token,
    ));
    link.report(connection::HubConnection::Connected);
    FleetBackend::remote_over(cfg(), fake.clone())
        .watching(link as Arc<dyn connection::ConnectionView>)
}

/// A real on-disk store; `Store`'s in-memory constructor is fleet-core-test
/// only. An `Arc` because `move_session`'s routed helper takes one (Transfer
/// 3c Task 3: `when: idle` on a busy source spawns a waiter that must
/// outlive the call) — every other routed helper still takes `&Mutex<Store>`
/// and gets there by deref coercion from `&Arc<Mutex<Store>>`.
///
/// Each one is a copy of a file migrated once per test process. Migrating a
/// fresh file takes ~70 ms, and the sweeps below open one store per routed
/// command, ~1,100 in all: that was 32 s of this crate's 33 s test run. A
/// copy opens in about a millisecond; `open_with_bus` still runs `migrate()`
/// on it, which finds nothing left to do.
fn store() -> (tempfile::TempDir, Arc<Mutex<Store>>) {
    static TEMPLATE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let template = TEMPLATE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        // Dropping the only connection checkpoints the WAL into the main
        // file and removes it, so the main file alone is the whole database.
        drop(Store::open_with_bus(&path, Arc::new(NoopEventBus)).unwrap());
        assert!(
            !dir.path().join("state.db-wal").exists(),
            "the template's WAL outlived its connection; a copy of the main file would miss it"
        );
        std::fs::read(&path).unwrap()
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    std::fs::write(&path, template).unwrap();
    let store = Store::open_with_bus(&path, Arc::new(NoopEventBus)).unwrap();
    (dir, Arc::new(Mutex::new(store)))
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
const RUN_OUTPUT_JSON: &str = r#"{"exit_code":0,"output":"Success","truncated":false}"#;
const DEBUG_DEVICE_JSON: &str = r#"{"id":3,"title":"Pixel 8","host":"mac","platform":"android","kind":"physical","key":"R5","name":"Pixel 8","state":"online","ready":true,"shared":false,"first_seen_at":1,"last_seen_at":2}"#;
const FORM_VIEW_JSON: &str = r#"{"form_id":"f_a","session_id":4,"host_alias":"h","title":"T","spec":{},"state":"pending","created_at":1}"#;
const SESSION_PAYLOAD: &str = r#"{"id":42,"tmux_name":"from-the-hub","host_alias":"hetzner","created_at":1,"last_activity_at":2,"status":"running","kind":"tmux","turn_seq":0,"tags":[]}"#;
/// `transport` is required on the wire (migration 034): it is a `String`,
/// not an `Option`, so `ok_json_compact` never strips it and a row without
/// it does not parse.
const HOST_PAYLOAD: &str =
    r#"{"alias":"trn","reachable":true,"hidden":false,"provisioned":true,"transport":"ssh"}"#;
/// A `ProjectTreeRow` as the hub answers `add_project`. `last_session_at` is
/// `None` and stripped, the way `ok_json_compact` sends it.
const PROJECT_TREE_PAYLOAD: &str = r#"{"project":{"id":7,"owner":"o","repo":"r","base_path":"/p/o/r","adopted":false,"system":false},"worktrees":[]}"#;
/// A `work_link { summarize }` answer (work graph M13.4c).
const SUMMARY_PAYLOAD: &str = r#"{"key":"ABC-1","link_id":4,"host_alias":"hetzner","claude_session_id":"0f8fad5b-d9cb-469f-a165-70867728950e","model":"haiku","journal_id":9,"at":1,"summary":"fenced"}"#;
const TASK_PAYLOAD: &str = r#"{"id":11,"state":"cancelled","created_at":1}"#;
/// A native item (`TASK-<id>`, shared work context), as `work_link
/// { create | accept | reject }` answers it.
const NATIVE_ITEM_PAYLOAD: &str = r#"{"id":9,"source":"local","key":"TASK-9","title":"Write notes","status_category":"todo","created_at":1,"updated_at":1,"origin":"manual"}"#;
/// A complete `MoveReport` wrapped as a `MoveOutcome::Moved` — all twelve
/// report fields plus the internal tag `"kind":"moved"`, the last field a
/// whole `SessionRow` (the same one as [`SESSION_PAYLOAD`]) whose own `kind`
/// (the session kind) sits one level down, untouched by the outcome's tag.
const MOVE_PAYLOAD: &str = r#"{"kind":"moved","source_session_id":7,"target_session_id":43,"from_host":"trn","to_host":"hetzner","tmux_name":"demo","claude_session_id":"abc","branch":"main","target_cwd":"/w/demo","transcript_bytes":1024,"source_killed":true,"warnings":[],"carried":{"commits":2,"bundle_bytes":1234,"dirty_entries":[{"status":" M","path":"src/lib.rs"}],"ignored_carried":[{"path":".env","bytes":4096}],"ignored_left_behind":[{"path":"node_modules/","bytes":null,"reason":"denylisted"}],"target_seeded":"existing","session_state":{"carried":[{"path":"subagents/agent-ab12.jsonl","bytes":2048}],"kept_target":[],"left_behind":[]},"memory":{"carried":[],"kept_target":["deploy.md"],"identical":3,"index_lines_added":0,"left_behind":[]}},"target":{"id":43,"tmux_name":"demo","host_alias":"hetzner","created_at":1,"last_activity_at":2,"status":"running","kind":"tmux","turn_seq":0,"tags":[]}}"#;
/// A complete `RepairReport` (`repair_session` uses `ok_json`, not the
/// null-stripping `ok_json_compact`, so every `Option` is present as a real
/// key — `null` included — and every non-`Option` field is required).
const REPAIR_PAYLOAD: &str = r#"{"session_id":7,"host_alias":"trn","tmux_name":"demo","project_root":"/p","cwd":"/p","cwd_physical":null,"healthy":true,"actions":[],"warnings":[],"needs_explicit_repair":false,"deferred":[],"branch_source":null,"tmux":null,"tmux_alive":true,"tmux_dead":false,"tmux_cwd_stale":false,"worktree_row_updated":false,"sibling_session_ids":[],"vanished_guard":null}"#;
/// A complete `ResolveMoveReport`: every field is required (no `Option`), so
/// this is the whole shape, not a null-stripped subset.
const RESOLVE_MOVE_PAYLOAD: &str = r#"{"action":"finish","source_session_id":7,"target_session_id":43,"from_host":"trn","to_host":"hetzner","source_killed":true,"target_killed":false,"warnings":[]}"#;
/// A complete `OperatorStatus`: both `Option` fields are required on the
/// wire (no `#[serde(default)]`), so `session` and `blocked` are spelled out
/// as `null` rather than omitted. `host` is the one defaulted field (a hub
/// from before it only ever homed the operator on `local`), spelled out here
/// all the same so the payload is the whole shape.
const OPERATOR_STATUS_PAYLOAD: &str =
    r#"{"ready":true,"session":null,"blocked":null,"host":"local"}"#;

/// One row of the tables below: the command it drives, the tool that command
/// must name, and the arguments it must send.
///
/// The closure hands back the command's own `Result`, and `check` requires it
/// to be `Ok`. It used to be discarded (`let _ = …`), which meant a payload
/// that could not deserialise into the command's return type still passed —
/// `move_session`'s case answered `"{}"` for a twelve-field `MoveReport` and
/// was green. With the result thrown away, "the same shape the local path
/// returns" was asserted by reading, not by test.
///
/// The command name is the first column so that `check` can hold the table's
/// tool — which is what the request really carried — against the tool
/// [`VERDICTS`] claims. The row is now what *drives* the tool
/// (`HubBackend::route` looks it up), so what this catches is a command
/// routing under somebody else's name: a copy-pasted `route("repo_file", …)`
/// inside `repo_diff` would send the wrong tool and nothing else would say
/// so, because both names are in the table.
type Case = (
    &'static str,
    &'static str,
    Value,
    &'static str,
    Box<dyn Fn(&FleetBackend, &Arc<Mutex<Store>>, &Arc<SshClient>) -> Result<(), IpcError>>,
);

fn check(cases: Vec<Case>) {
    for (command, tool, want_args, payload, run) in cases {
        let fake = Fake::answering(payload);
        let (_dir, st) = store();
        let got = run(&remote_backend(&fake), &st, &ssh());
        let (got_tool, got_args) = fake.only_call();
        assert_eq!(got_tool, tool, "wrong tool for {tool}");
        assert_eq!(got_args, want_args, "wrong arguments for {tool}");
        // The table's row is a claim about the same call this case just
        // recorded. `set_session_friendly_name` -> `set_friendly_name` is one
        // of the two places the two vocabularies differ (`health_check` ->
        // `fleet_health` is the other), and it is checked here like any other
        // row rather than excused.
        assert_eq!(
            verdicts::verdict(command).and_then(Verdict::tool),
            Some(got_tool.as_str()),
            "{command} sent {got_tool}, but its VERDICTS row names {:?} — that row is \
             published to the frontend and the docs",
            verdicts::verdict(command).and_then(Verdict::tool),
        );
        if let Err(e) = got {
            panic!(
                "{tool}: the hub's answer did not come back as the command's return \
                 type, so the frontend would get an error where the local path \
                 returns a value: {e:?}"
            );
        }
    }
}

/// Routed commands that no row of the two tables drives, each with the test
/// that does [`check`]'s job for it instead. An empty list would be better; a
/// silent gap would be worse, because a row whose tool nothing exercises is a
/// row whose tool nothing checks.
///
/// The named test must do what `check` does — hold the row's `tool` against
/// the tool the recorded request carried. "It asserts the tool" is not
/// enough: asserting a wire value against a second hand-typed literal leaves
/// the row itself unchecked, which is how the first version of this list was
/// wrong.
const ROUTED_WITHOUT_A_CASE: &[(&str, &str)] = &[
    (
        "debug_device_screenshot",
        "a_hub_screenshot_comes_back_as_the_image_block_it_answered holds its \
         VERDICTS row against the tool the request carried, the same way check \
         does; it is not a case because its answer is an image block, which the \
         fake's text-only answer cannot carry",
    ),
    (
        "save_download",
        "a_hub_download_is_checked_through_list_downloads_before_a_byte_moves \
         holds its VERDICTS row against the tool the request carried, the same \
         way check does; it is not a case because its second half is an HTTP \
         GET the fake transport does not answer",
    ),
    (
        "health_check",
        "health_is_the_hubs_fleet_not_this_apps_empty_database asserts its empty \
     arguments and cross-checks its VERDICTS row against the tool the request \
     carried, the same way check does; it is not a case because it also needs \
     a seeded local store to prove the answer is not the local one",
    ),
];

/// The other half of the tool check in [`check`]: a wrong tool must not be
/// able to hide by having no case at all.
/// Work graph M5: every Routed work command is one fleet-core's isolation
/// matrix knows (`ROUTED_WORK_COMMANDS`, whose actions the matrix runs for
/// every caller), and the list names nothing this table does not route.
#[test]
fn every_routed_work_command_is_in_the_isolation_matrix() {
    let table: BTreeSet<(&str, &str)> = VERDICTS
        .iter()
        .filter_map(|(cmd, v)| match v {
            Verdict::Routed { tool } if matches!(*tool, "work" | "work_link" | "work_admin") => {
                Some((*cmd, *tool))
            }
            _ => None,
        })
        .collect();
    let listed: BTreeSet<(&str, &str)> = fleet_core::service::work::ROUTED_WORK_COMMANDS
        .iter()
        .map(|(cmd, tool, _)| (*cmd, *tool))
        .collect();
    assert_eq!(
        table, listed,
        "a Routed work command without an isolation-matrix entry (or a stale entry): add it \
         to fleet_core::service::work::ROUTED_WORK_COMMANDS"
    );
}

#[test]
fn every_routed_row_is_driven_by_a_case() {
    let driven: BTreeSet<&str> = routed_read_cases()
        .iter()
        .chain(routed_mutation_cases().iter())
        .map(|(command, ..)| *command)
        .collect();
    let excused: BTreeSet<&str> = ROUTED_WITHOUT_A_CASE.iter().map(|(n, _)| *n).collect();

    let undriven: Vec<&str> = VERDICTS
        .iter()
        .filter(|(_, v)| v.tool().is_some())
        .map(|(name, _)| *name)
        .filter(|name| !driven.contains(name) && !excused.contains(name))
        .collect();
    assert!(
        undriven.is_empty(),
        "these commands route on the backend but no case drives them, so the \
         tool their VERDICTS row names is a literal nothing checks:\n  {}\n\n\
         Add a case to routed_read_cases/routed_mutation_cases, or add the \
         command to ROUTED_WITHOUT_A_CASE with the test that covers it.",
        undriven.join("\n  ")
    );

    let stale: Vec<&str> = excused
        .iter()
        .copied()
        .filter(|name| driven.contains(name))
        .collect();
    assert!(
        stale.is_empty(),
        "ROUTED_WITHOUT_A_CASE excuses commands the tables now drive: {stale:?}"
    );
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
    let mut cases = routed_read_cases_but_org_admin();
    cases.extend(org_admin_read_cases());
    cases
}

fn routed_read_cases_but_org_admin() -> Vec<Case> {
    use fleet_core::service::repo::SessionIdArgs;
    use fleet_core::service::repo_read::{
        DiffRange, RepoCommitArgs, RepoCommitDiffArgs, RepoFileArgs, RepoLogArgs, RepoRangeDiffArgs,
    };
    use fleet_core::service::worktrees::ListHostWorktreesArgs;

    vec![
        (
            "list_sessions",
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
            "queued_prompts",
            "queued_prompts",
            json!({ "session_id": 7 }),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::queued_prompts(
                    b,
                    fleet_core::service::sessions::QueuedPromptsArgs { session_id: 7 },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "agent_installs",
            "agent_installs",
            json!({ "alias": "trn" }),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::hosts::routed::agent_installs(
                    b,
                    fleet_core::service::agent_install::AgentInstallsArgs {
                        alias: Some("trn".into()),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "list_hosts",
            "list_hosts",
            json!({}),
            "[]",
            Box::new(|b, s, _| block_on(commands::hosts::routed::list_hosts(b, s)).map(|_| ())),
        ),
        (
            "list_accounts",
            "list_accounts",
            json!({}),
            "[]",
            Box::new(|b, s, _| block_on(commands::hosts::routed::list_accounts(b, s)).map(|_| ())),
        ),
        (
            "list_account_usage",
            "account_usage",
            json!({}),
            "[]",
            Box::new(|b, s, _| {
                let cache = Mutex::new(fleet_core::service::account_usage::UsageCache::new());
                block_on(commands::account_usage::routed::list_account_usage(
                    b, s, &cache,
                ))
                .map(|_| ())
            }),
        ),
        (
            "check_account_headroom",
            "check_account_headroom",
            json!({ "host_alias": "mac", "profile": "work" }),
            r#"{"pause_at_pct":90.0,"over":false,"logins":[]}"#,
            Box::new(|b, s, _| {
                let cache = Mutex::new(fleet_core::service::account_usage::UsageCache::new());
                let args = fleet_core::service::account_limits::CheckAccountHeadroomArgs {
                    host_alias: "mac".into(),
                    profile: Some("work".into()),
                };
                block_on(commands::account_usage::routed::check_account_headroom(
                    b, args, s, &cache,
                ))
                .map(|_| ())
            }),
        ),
        (
            "list_projects",
            "list_projects",
            json!({ "summary": false }),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::projects::routed::list_projects(b, s)).map(|_| ())
            }),
        ),
        (
            "project_picks",
            "project_picks",
            json!({}),
            r#"[{"owner":"o","repo":"r","pinned":true,"vis":"keep","grp":"tools"}]"#,
            Box::new(|b, s, _| {
                let v = block_on(commands::projects::routed::project_picks(b, s))?;
                assert!(v[0].pinned, "the hub's answer");
                assert_eq!(v[0].grp.as_deref(), Some("tools"));
                Ok(())
            }),
        ),
        (
            "refresh_projects",
            "refresh_projects",
            json!({}),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::projects::routed::refresh_projects(b, s)).map(|_| ())
            }),
        ),
        (
            "list_github_repos",
            "list_github_repos",
            json!({ "host_alias": "trn" }),
            r#"[{"name_with_owner":"acme/widget","is_private":true}]"#,
            Box::new(|b, s, h| {
                block_on(commands::projects::routed::list_github_repos(
                    b,
                    commands::projects::ListGithubReposArgs {
                        host_alias: "trn".into(),
                        owner: None,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "list_github_repos",
            "list_github_repos",
            json!({ "host_alias": "trn", "owner": "papaya-pos" }),
            r#"[{"name_with_owner":"papaya-pos/receipts","is_private":true}]"#,
            Box::new(|b, s, h| {
                block_on(commands::projects::routed::list_github_repos(
                    b,
                    commands::projects::ListGithubReposArgs {
                        host_alias: "trn".into(),
                        owner: Some("papaya-pos".into()),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        // Both fields are required on both sides, so the command's own
        // argument struct IS the wire: no defaulted key, no clamped value,
        // nothing sent only when set. This case pins that identity — a field
        // added to one side and not the other would change the recorded
        // arguments here.
        (
            "list_host_worktrees",
            "list_host_worktrees",
            json!({ "host_alias": "hetzner", "project_id": 4 }),
            r#"{"host_alias":"hetzner","project_id":4,"cloned":true,"worktrees":[{"id":9,"project_id":4,"host_alias":"hetzner","name":"main","path":"/w/r"}]}"#,
            Box::new(|b, s, sh| {
                block_on(commands::worktrees::routed::list_host_worktrees(
                    b,
                    ListHostWorktreesArgs {
                        host_alias: "hetzner".into(),
                        project_id: 4,
                    },
                    s,
                    sh,
                ))
                .map(|_| ())
            }),
        ),
        // Both quick-reply commands are the same tool: the read sends no
        // `set` key, the write sends the list. Pinned here so a later
        // "tidy" that makes the read send `set: null` — which the tool
        // would read as "replace with nothing" — fails instead of wiping a
        // fleet's chips the first time a paired desktop opened a composer.
        (
            "quick_replies",
            "quick_replies",
            json!({}),
            r#"[{"label":"Clear","text":"/clear"}]"#,
            Box::new(|b, s, _| {
                block_on(commands::quick_replies::routed::quick_replies(b, s)).map(|_| ())
            }),
        ),
        (
            "set_quick_replies",
            "quick_replies",
            json!({ "set": [{ "label": "Tests", "text": "run the tests" }] }),
            r#"[{"label":"Tests","text":"run the tests"}]"#,
            Box::new(|b, s, _| {
                block_on(commands::quick_replies::routed::set_quick_replies(
                    b,
                    vec![fleet_core::service::quick_replies::QuickReply {
                        label: "Tests".into(),
                        text: "run the tests".into(),
                        auto_send: None,
                    }],
                    None,
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // File downloads: the hub keeps the copies, so the list, a send and
        // a removal are its tools.
        (
            "list_downloads",
            "list_downloads",
            json!({ "session_id": 4 }),
            r#"{"downloads":[{"id":7,"at":1,"host_alias":"trn","path":"/w/a.pdf","name":"a.pdf","size":3,"state":"ready","source":"agent"}],"total_bytes":3,"max_total_bytes":10,"max_file_bytes":5}"#,
            Box::new(|b, s, _| {
                block_on(commands::downloads::routed::list_downloads(
                    b,
                    fleet_core::service::downloads::ListDownloadsArgs {
                        session_id: Some(4),
                        limit: None,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "send_file",
            "send_file",
            json!({ "session_id": 4, "path": "out/a.pdf", "note": "the report" }),
            r#"{"id":7,"at":1,"host_alias":"trn","session_id":4,"path":"/w/out/a.pdf","name":"a.pdf","size":3,"state":"fetching","source":"person","note":"the report"}"#,
            Box::new(|b, s, ssh| {
                block_on(commands::downloads::routed::send_file(
                    b,
                    fleet_core::service::downloads::SendFileArgs {
                        session_id: 4,
                        path: "out/a.pdf".into(),
                        note: Some("the report".into()),
                    },
                    s,
                    ssh,
                ))
                .map(|_| ())
            }),
        ),
        (
            "remove_download",
            "remove_download",
            json!({ "id": 7 }),
            r#"{"removed":true}"#,
            Box::new(|b, s, _| {
                block_on(commands::downloads::routed::remove_download(b, 7, s)).map(|_| ())
            }),
        ),
        // Control's Library: one hub tool, by action.
        (
            "list_library",
            "library",
            json!({ "action": "list", "host_alias": "trn", "limit": 5 }),
            r#"{"items":[{"id":3,"at":1,"kind":"upload","host_alias":"trn","session_id":4,"path":"/w/a.pdf","name":"a.pdf","size":3}]}"#,
            Box::new(|b, s, _| {
                block_on(commands::library::routed::list_library(
                    b,
                    fleet_core::service::library::ListArgs {
                        session_id: None,
                        host_alias: Some("trn".into()),
                        limit: Some(5),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "add_library_items",
            "library",
            json!({ "action": "add", "kind": "upload", "session_id": 4, "files": [{ "path": "/w/a.pdf", "name": "a.pdf", "size": 3 }] }),
            r#"{"items":[]}"#,
            Box::new(|b, s, _| {
                block_on(commands::library::routed::add_library_items(
                    b,
                    fleet_core::service::library::AddArgs {
                        kind: "upload".into(),
                        session_id: 4,
                        files: vec![fleet_core::service::library::LibraryFile {
                            path: "/w/a.pdf".into(),
                            name: Some("a.pdf".into()),
                            size: Some(3),
                        }],
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "remove_library_item",
            "library",
            json!({ "action": "remove", "id": 3 }),
            r#"{"removed":true}"#,
            Box::new(|b, s, _| {
                block_on(commands::library::routed::remove_library_item(b, 3, s)).map(|_| ())
            }),
        ),
        (
            "list_tasks",
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
            "session_work_links",
            "work",
            json!({ "session_id": 7, "key": null }),
            // `work` answers null-stripped rows.
            r#"[{"id":1,"ref_key":"ABC-1","state":"confirmed","source":"manual","is_primary":true,"created_at":1}]"#,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::session_work_links(
                    b,
                    fleet_core::service::work::WorkArgs {
                        session_id: Some(7),
                        ..Default::default()
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_resume_plan",
            "work",
            json!({ "session_id": null, "key": "ABC-1", "action": "resume_plan",
                    "host_alias": "h", "with_brief": true }),
            r#"{"key":"ABC-1","modes":[{"mode":"last","ok":true}]}"#,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::work_resume_plan(
                    b,
                    commands::work::WorkResumePlanArgs {
                        key: "ABC-1".into(),
                        link_id: None,
                        host_alias: Some("h".into()),
                        with_brief: true,
                    },
                    s,
                    &ssh(),
                ))
                .map(|_| ())
            }),
        ),
        // Declarative pages P6: the fleet's settings are the hub's.
        (
            "get_fleet_settings",
            "get_settings",
            json!({}),
            r#"{"gc.enabled":"true","work.recent_days":"7"}"#,
            Box::new(|b, s, _| {
                let m = block_on(commands::pages::routed::get_fleet_settings(b, s))?;
                assert_eq!(m.get("gc.enabled").map(String::as_str), Some("true"));
                Ok(())
            }),
        ),
        (
            "describe_fleet_settings",
            "get_settings",
            json!({ "describe": true }),
            r#"[{"key":"work.recent_days","label":"Recent work","value":"7"}]"#,
            Box::new(|b, s, _| {
                let v = block_on(commands::pages::routed::describe_fleet_settings(b, s))?;
                assert_eq!(v[0]["value"], "7", "the hub's value, not this app's");
                Ok(())
            }),
        ),
        (
            "setting_proposals",
            "setting_proposals",
            json!({}),
            r#"{"can_write":false,"proposals":[{"id":4,"at":1,"key":"work.recent_days","value":"3","before":"14","source":"agent","state":"pending","current":"14"}]}"#,
            Box::new(|b, s, _| {
                let p = block_on(commands::pages::routed::setting_proposals(b, s))?;
                assert!(
                    !p.can_write,
                    "the hub decides whether this device may write"
                );
                assert_eq!(p.proposals[0].row.id, 4);
                Ok(())
            }),
        ),
        (
            "list_guides",
            "guide",
            json!({ "action": "list" }),
            r#"{"guides":[],"proposals":[],"can_write":false}"#,
            Box::new(|b, s, _| {
                let v = block_on(commands::pages::routed::list_guides(b, s))?;
                assert!(
                    !v.can_write,
                    "the hub decides whether this device may approve"
                );
                Ok(())
            }),
        ),
        (
            "list_forms",
            "ask",
            json!({ "list": { "session_id": 4, "state": "pending" } }),
            r#"[]"#,
            Box::new(|b, s, _| {
                block_on(commands::forms::routed::list_forms(
                    b,
                    s,
                    Some(4),
                    Some("pending".into()),
                ))
                .map(|_| ())
            }),
        ),
        // `leaving` is skipped when false, so a heartbeat is the session id
        // alone; the hub learns the device from the connection, never here.
        (
            "session_presence",
            "session_presence",
            json!({ "session_id": 42 }),
            r#"{"session_id":42,"viewers":[{"person_id":3,"name":"jane","since":1700000000}],"heartbeat_secs":20}"#,
            Box::new(|b, _, _| {
                block_on(commands::presence::routed::session_presence(
                    b,
                    fleet_core::service::presence::SessionPresenceArgs {
                        session_id: 42,
                        leaving: false,
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "list_pull_requests",
            "prs",
            json!({ "action": "list", "state": "open" }),
            r#"{"items":[],"total":0}"#,
            Box::new(|b, s, _| {
                block_on(commands::prs::routed::list_pull_requests(
                    b,
                    s,
                    fleet_core::service::prs::PrsArgs {
                        action: "list".into(),
                        state: Some("open".into()),
                        ..Default::default()
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "start_rules",
            "start_rules",
            json!({ "action": "accept", "rule_id": 3 }),
            r#"{"id":3,"pattern":"PD-*","project_id":2,"state":"active","confirmations":5,"hits":0,"created_at":1,"updated_at":2,"may_change":true}"#,
            Box::new(|b, s, _| {
                block_on(commands::start_rules::routed::start_rules(
                    b,
                    s,
                    fleet_core::service::start_rules::StartRulesArgs {
                        action: "accept".into(),
                        rule_id: Some(3),
                        rule: None,
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "control_handoffs",
            "control_handoffs",
            json!({ "limit": 5 }),
            r#"[{"id":1,"at":1,"kind":"session","tool":"send_prompt","session_id":3}]"#,
            Box::new(|b, s, _| {
                block_on(commands::mcp::routed::control_handoffs(b, s, Some(5))).map(|_| ())
            }),
        ),
        (
            "mcp_pending_confirms",
            "mcp_confirms",
            json!({}),
            r#"[{"nonce":"n","tool":"kill_session","summary":"","caller":"client:ux-agent","operator":true,"asked_at":1}]"#,
            Box::new(|b, _, _| {
                let guards = fleet_core::mcp::McpGuards::new(Arc::new(|_| {}));
                block_on(commands::mcp::routed::mcp_pending_confirms(b, &guards)).map(|_| ())
            }),
        ),
        (
            "list_debug_devices",
            "debug_devices",
            json!({ "action": "list" }),
            r#"{"devices":[],"hosts":[]}"#,
            Box::new(|b, s, ssh| {
                block_on(commands::debug_devices::routed::list_debug_devices(
                    b, s, ssh,
                ))
                .map(|_| ())
            }),
        ),
        (
            "list_peer_links",
            "list_peer_links",
            json!({}),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::federation::routed::list_peer_links(b, s)).map(|_| ())
            }),
        ),
        (
            "list_update_targets",
            "update_status",
            json!({}),
            r#"{"targets":[]}"#,
            Box::new(|b, s, _| {
                block_on(commands::updates::routed::list_update_targets(b, s)).map(|_| ())
            }),
        ),
        (
            "list_runs",
            "runs",
            json!({ "action": "list", "since": 5, "kind": "jev", "limit": 20 }),
            r#"{"runs":[{"id":"jev:1","source":"jev","kind":"jev","owner":"status_map","started_at":6,"outcome":"ok","session_ids":[]}],"total":1}"#,
            Box::new(|b, s, _| {
                block_on(commands::runs::routed::list_runs(
                    b,
                    s,
                    fleet_core::service::runs::RunsArgs {
                        since: Some(5),
                        kind: Some("jev".into()),
                        limit: Some(20),
                        ..Default::default()
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "routines",
            "routines",
            json!({ "action": "failing" }),
            r#"[]"#,
            Box::new(|b, s, ssh| {
                block_on(commands::routines::routed::routines(
                    b,
                    s,
                    ssh,
                    &fleet_core::cancel::CancellationRegistry::new(),
                    commands::routines::RoutinesArgs {
                        action: "failing".into(),
                        ..Default::default()
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "get_form",
            "ask",
            json!({ "get": "f_a" }),
            FORM_VIEW_JSON,
            Box::new(|b, s, _| {
                block_on(commands::forms::routed::get_form(b, s, "f_a".into())).map(|_| ())
            }),
        ),
        (
            "setting_history",
            "setting_history",
            json!({ "key": "work.recent_days", "limit": null }),
            r#"[{"id":1,"at":1,"key":"work.recent_days","after":"3","actor":"person","actor_detail":"client laptop"}]"#,
            Box::new(|b, s, _| {
                block_on(commands::pages::routed::setting_history(
                    b,
                    s,
                    "work.recent_days".into(),
                    None,
                ))
                .map(|_| ())
            }),
        ),
        (
            "list_trackers",
            "work",
            json!({ "session_id": null, "key": null, "action": "trackers" }),
            r#"[{"id":1,"provider":"jira","name":"acme","site_url":"https://acme.atlassian.net","state":"ok","created_at":1}]"#,
            Box::new(|b, s, _| {
                block_on(commands::trackers::routed::list_trackers(b, s)).map(|_| ())
            }),
        ),
        (
            "list_orgs",
            "work",
            json!({ "session_id": null, "key": null, "action": "orgs" }),
            r#"[{"id":1,"name":"Company A","created_at":1,"rules":[{"id":2,"org_id":1,"owner":"acme"}],"hosts":["h"],"trackers":[]}]"#,
            Box::new(|b, s, _| block_on(commands::orgs::routed::list_orgs(b, s)).map(|_| ())),
        ),
        (
            "org_suggestions",
            "work",
            json!({ "session_id": null, "key": null, "action": "org_suggestions" }),
            r#"[{"name":"acme","owner":"acme","sessions":2,"reason":"2 live sessions under acme/*"}]"#,
            Box::new(|b, s, _| block_on(commands::orgs::routed::org_suggestions(b, s)).map(|_| ())),
        ),
        (
            "work_tickets",
            "work",
            json!({ "session_id": null, "key": null, "action": "tickets",
                    "view": "mine", "query": "login", "limit": 20, "include_local": true }),
            r#"[{"id":3,"source":"jira","key":"ABC-1","title":"Login","status_category":"todo","created_at":1,"updated_at":1}]"#,
            Box::new(|b, s, _| {
                block_on(commands::trackers::routed::work_tickets(
                    b,
                    commands::trackers::WorkTicketsArgs {
                        tracker_id: None,
                        view: Some("mine".into()),
                        query: Some("login".into()),
                        limit: Some(20),
                        include_local: Some(true),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_lookup",
            "work",
            json!({ "session_id": null, "key": null, "action": "lookup",
                    "url": "https://acme.atlassian.net/browse/ABC-1" }),
            r#"{"id":3,"source":"jira","key":"ABC-1","title":"Login","status_category":"todo","created_at":1,"updated_at":1}"#,
            Box::new(|b, s, _| {
                block_on(commands::trackers::routed::work_lookup(
                    b,
                    commands::trackers::WorkLookupArgs {
                        reference: "https://acme.atlassian.net/browse/ABC-1".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_ticket_card",
            "work",
            json!({ "session_id": null, "key": "PAY-7", "action": "card" }),
            r#"{"key":"PAY-7","title":"Refund","cached":true,"acceptance":["Refund issued"],"composer_text":"Ticket PAY-7: Refund\n"}"#,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::work_ticket_card(
                    b,
                    commands::work::WorkTicketCardArgs {
                        key: "PAY-7".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_today",
            "work",
            json!({ "session_id": null, "key": null, "action": "today", "since": 1_700_000_000 }),
            r#"{"since":1700000000,"now":1700003600,"groups":[{"bucket":"waiting","key":"PAY-7","title":"Refund","sessions":[{"id":4,"name":"pay","host_alias":"h","attention":"waiting","last_activity_at":1700003000}]}],"shipped":[]}"#,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::work_today(
                    b,
                    commands::work::WorkTodayArgs {
                        since: Some(1_700_000_000),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_purge_impact",
            "work",
            json!({ "session_id": null, "key": null, "action": "purge_impact",
                    "project_id": 3, "host_aliases": ["h"] }),
            r#"{"keys":["ABC-1"]}"#,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::work_purge_impact(
                    b,
                    commands::work::WorkPurgeImpactArgs {
                        project_id: 3,
                        host_aliases: vec!["h".into()],
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_tidy",
            "work",
            json!({ "session_id": null, "key": null, "action": "tidy" }),
            r#"{"candidates":[],"auto_tidy":false,"auto_reasons":[],"done_days":2,"idle_hours":4}"#,
            Box::new(|b, s, _| block_on(commands::work::routed::work_tidy(b, s)).map(|_| ())),
        ),
        (
            "work_reopened",
            "work",
            json!({ "session_id": null, "key": null, "action": "reopened" }),
            r#"[{"item_id":3,"key":"ABC-1","title":"Login","reopened_at":5,"past_sessions":2}]"#,
            Box::new(|b, s, _| block_on(commands::work::routed::work_reopened(b, s)).map(|_| ())),
        ),
        (
            "name_session_work",
            "work_link",
            json!({ "session_id": 7, "action": "name", "key": "OPS", "item_id": null,
                    "link_id": null, "source": null, "title": "Ops cleanup" }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::name_session_work(
                    b,
                    commands::work::NameSessionWorkArgs {
                        session_id: 7,
                        title: "Ops cleanup".into(),
                        key: Some("OPS".into()),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "rename_work_item",
            "work_link",
            json!({ "session_id": null, "action": "name", "key": null, "item_id": 3,
                    "link_id": null, "source": null, "title": "Ops, renamed" }),
            r#"{"id":3,"source":"local","key":"OPS","title":"Ops, renamed","status_category":"todo","created_at":1,"updated_at":2}"#,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::rename_work_item(
                    b,
                    commands::work::RenameWorkItemArgs {
                        item_id: 3,
                        title: "Ops, renamed".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // Shared work context (design 2026-09-29).
        (
            "create_work_task",
            "work_link",
            json!({ "session_id": null, "action": "create", "key": null, "item_id": null,
                    "link_id": null, "source": null, "title": "Write notes",
                    "parent": "item:5", "project_id": 3, "notes": "v1" }),
            NATIVE_ITEM_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::create_work_task(
                    b,
                    commands::work::CreateWorkTaskArgs {
                        title: "Write notes".into(),
                        parent: Some("item:5".into()),
                        project_id: Some(3),
                        notes: Some("v1".into()),
                        assignees: None,
                        due_at: None,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // The board (sprints design 2026-09-28 §6c).
        (
            "set_work_status",
            "work_link",
            json!({ "session_id": null, "action": "set_status", "key": null, "item_id": 9,
                    "link_id": null, "source": null, "status": "in_progress" }),
            NATIVE_ITEM_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::set_work_status(
                    b,
                    commands::work::SetWorkStatusArgs {
                        item_id: 9,
                        status: "in_progress".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // Task editing.
        (
            "edit_work_item",
            "work_link",
            json!({ "session_id": null, "action": "edit", "key": null, "item_id": 9,
                    "link_id": null, "source": null, "title": "Fix login",
                    "notes": "", "assignees": ["Ana"], "due_at": "2026-10-16" }),
            NATIVE_ITEM_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::edit_work_item(
                    b,
                    commands::work::EditWorkItemArgs {
                        item_id: 9,
                        title: Some("Fix login".into()),
                        notes: Some(String::new()),
                        assignees: Some(vec!["Ana".into()]),
                        due_at: Some("2026-10-16".into()),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "accept_work_proposal",
            "work_link",
            json!({ "session_id": null, "action": "accept", "key": null, "item_id": 9,
                    "link_id": null, "source": null }),
            NATIVE_ITEM_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::decide_work_proposal(
                    b,
                    commands::work::WorkProposalArgs {
                        item_id: 9,
                        merge_into: None,
                    },
                    true,
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "reject_work_proposal",
            "work_link",
            json!({ "session_id": null, "action": "reject", "key": null, "item_id": 9,
                    "link_id": null, "source": null }),
            NATIVE_ITEM_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::decide_work_proposal(
                    b,
                    commands::work::WorkProposalArgs {
                        item_id: 9,
                        merge_into: None,
                    },
                    false,
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "session_history",
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
            "session_activity",
            "session_activity",
            json!({ "session_id": 7 }),
            r#"{"claude_status":"working","current_activity":null,"stuck_kind":null,"waiting_for":null,"spinner":"Cooking… (3s)"}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::session_activity(
                    b,
                    commands::sessions::SessionActivityArgs { session_id: 7 },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "session_conversation",
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
            "session_tool_detail",
            "session_tool_detail",
            json!({ "session_id": 7, "tool_use_id": "toolu_1" }),
            r#"{"id":"toolu_1","name":"Bash","input":"{}","edit":null,"command":"ls","result":"a","is_error":false}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::session_tool_detail(
                    b,
                    commands::sessions::SessionToolDetailArgs {
                        session_id: 7,
                        tool_use_id: "toolu_1".into(),
                        claude_session_id: None,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "session_conversations",
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
        // `all`/`limit`/`skip` carry `#[serde(default)]` for the webview, so
        // the History view can leave them out — and the desktop still SENDS
        // the zeroes it defaulted them to, because the tool's own defaults
        // (`all: true`, `limit: 50`) are not the desktop's. An argument
        // quietly omitted here would change what the History view shows, so
        // the zero shape is pinned beside the populated one.
        (
            "repo_log",
            "repo_log",
            json!({ "session_id": 7, "all": false, "limit": 0, "skip": 0 }),
            "[]",
            Box::new(|b, s, h| {
                block_on(commands::history::routed::repo_log(
                    b,
                    RepoLogArgs {
                        session_id: 7,
                        all: false,
                        limit: 0,
                        skip: 0,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "repo_branches",
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
            "repo_branch_diff",
            "repo_branch_diff",
            json!({ "session_id": 7 }),
            r#"{"branch":"feat","upstream":null,"unpushed":[],"unpushedFiles":[],"truncated":false,"base":"origin/main","aheadOfBase":2,"baseFiles":[]}"#,
            Box::new(|b, s, h| {
                block_on(commands::files::routed::repo_branch_diff(
                    b,
                    SessionIdArgs { session_id: 7 },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "repo_range_diff",
            "repo_range_diff",
            json!({ "session_id": 7, "path": "src/lib.rs", "range": "base" }),
            r#"{"path":"src/lib.rs","diff":"","binary":false,"truncated":false}"#,
            Box::new(|b, s, h| {
                block_on(commands::files::routed::repo_range_diff(
                    b,
                    RepoRangeDiffArgs {
                        session_id: 7,
                        path: "src/lib.rs".into(),
                        range: DiffRange::Base,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "repo_tree",
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
        (
            "repo_blame",
            "repo_blame",
            json!({ "session_id": 7, "path": "src/lib.rs" }),
            r#"{"path":"src/lib.rs","hunks":[],"truncated":false}"#,
            Box::new(|b, s, h| {
                block_on(commands::files::routed::repo_blame(
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
        // The simple `(b, s, _)` shape: `operator_status` needs neither ssh
        // nor a cancellation registry, unlike `ensure_operator` below.
        (
            "operator_status",
            "operator_status",
            json!({}),
            OPERATOR_STATUS_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::operator::routed::operator_status(b, s)).map(|_| ())
            }),
        ),
        // The Assets overview on a hub client: the hub's catalog listing, and
        // the scan that refreshes it. Everything else in the catalog refuses.
        (
            "catalog_list_assets",
            "list_assets",
            // Assets M5 fix round 1: this desktop asks for every catalog; an
            // older one sends `{}` and gets personal's listing.
            json!({ "all_catalogs": true }),
            r#"{"head":"abc","loaded_at":1,"assets":[{"kind":"skill","name":"worktree","version":"1","description":"d","tags":[],"hosts":[{"host_alias":"nas","harness":"claude","state":"in_sync"}]}],"unmanaged":[{"host_alias":"nas","harness":"claude","kind":"skill","name":"extra","state":"unmanaged","scanned_at":1,"managed":false}],"problems":[]}"#,
            Box::new(|b, s, _| {
                block_on(commands::assets::routed::catalog_list_assets(b, s)).map(|_| ())
            }),
        ),
        (
            "assets_scan_hosts",
            "scan_assets",
            json!({ "host_alias": "nas" }),
            r#"[{"host":"nas","status":"scanned","rows":3}]"#,
            Box::new(|b, s, h| {
                block_on(commands::assets::routed::assets_scan_hosts(
                    b,
                    s,
                    h,
                    Some("nas".into()),
                ))
                .map(|_| ())
            }),
        ),
        // Work graph M14: the Work view's reads.
        (
            "work_tree",
            "work",
            json!({ "session_id": null, "key": null, "action": "tree", "limit": 25,
                    "cursor": "c1", "per_task": 3,
                    "filters": { "org": "none", "status": "open", "has": "active" },
                    "sections": [{ "org_id": 1, "group_id": "label:Payments", "limit": 50 }],
                    "with_review_total": true }),
            r#"{"tasks":[],"groups":[],"orgs":[],"trackers":[],"total":0,"generated_at":1}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::work_tree(
                    b,
                    commands::work_view::WorkTreeCmdArgs {
                        filters: Some(fleet_core::service::work::view::WorkTreeFilters {
                            org: Some(fleet_core::service::work::view::IdOrWord::Word(
                                "none".into(),
                            )),
                            status: Some("open".into()),
                            has: Some("active".into()),
                            ..Default::default()
                        }),
                        cursor: Some("c1".into()),
                        limit: Some(25),
                        per_task: Some(3),
                        sections: Some(vec![fleet_core::service::work::view::SectionAsk {
                            org_id: Some(1),
                            group_id: "label:Payments".into(),
                            limit: Some(50),
                        }]),
                        with_review_total: Some(true),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_task",
            "work",
            json!({ "session_id": null, "key": null, "action": "task", "task_id": "item:3" }),
            r#"{"task":{"task_id":"item:3","title":"t","kind":"local","unavailable":false,"mine":false,"org_source":"none","org_fenced":false,"org_mixed":false,"group":{"id":"none","label":"No group","source":"none","editable":true},"counts":{"active":0,"ended":0,"suggested":0},"needs_you":false,"review":false,"placement_version":0,"sessions":[],"sessions_more":0}}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::work_task(
                    b,
                    commands::work_view::WorkTaskCmdArgs {
                        task_id: "item:3".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_missions",
            "work",
            json!({ "session_id": null, "key": null, "action": "missions" }),
            r#"[{"id":4,"name":"m","goal":"g","mode":"finite","state":"draft","level":0,"plan_version":1,"created_at":1,"updated_at":1,"version":1}]"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::work_missions(
                    b,
                    commands::missions::WorkMissionsArgs {},
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_mission",
            "work",
            json!({ "session_id": null, "key": null, "action": "mission", "mission_id": 4,
                    "before_event": 90 }),
            r#"{"mission":{"id":4,"name":"m","goal":"g","mode":"finite","state":"draft","level":0,"plan_version":1,"created_at":1,"updated_at":1,"version":1}}"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::work_mission(
                    b,
                    commands::missions::WorkMissionArgs {
                        mission_id: 4,
                        before_event: Some(90),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_session_tasks",
            "work",
            json!({ "session_id": 7, "key": null, "action": "session_tasks" }),
            r#"{"session_id":7,"links":[]}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::work_session_tasks(
                    b,
                    commands::work_view::WorkSessionTasksArgs { session_id: 7 },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_review",
            "work",
            json!({ "session_id": null, "key": null, "action": "review", "limit": 10 }),
            r#"{"items":[],"total":0}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::work_review(
                    b,
                    commands::work_view::WorkReviewArgs {
                        cursor: None,
                        limit: Some(10),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_rules",
            "work",
            json!({ "session_id": null, "key": null, "action": "rules" }),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::work_rules(
                    b,
                    commands::work_view::NoArgs {},
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_rule_preview",
            "work",
            json!({ "session_id": null, "key": null, "action": "rule_preview",
                    "rule": { "name": "Pay", "conditions": { "key_prefix": "PAY" }, "group": "Payments" } }),
            r#"{"affected":[],"total":0,"kept_manual":0}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::work_rule_preview(
                    b,
                    commands::work_view::WorkRuleArgs {
                        rule: fleet_core::service::work::structure::RuleInput {
                            name: "Pay".into(),
                            conditions: fleet_core::store::RuleConditions {
                                key_prefix: Some("PAY".into()),
                                ..Default::default()
                            },
                            group: "Payments".into(),
                            ..Default::default()
                        },
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_views",
            "work",
            json!({ "session_id": null, "key": null, "action": "views" }),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::work_views(
                    b,
                    commands::work_view::NoArgs {},
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "work_org_impact",
            "work",
            json!({ "session_id": null, "key": null, "action": "org_impact",
                    "task_id": "item:3", "org_id": 0 }),
            r#"{"task_id":"item:3","allowed":false,"reason":"same_org","links":[],"hosts_losing":[],"hosts_gaining":[],"bound_clients_losing":0,"bound_clients_gaining":0,"journal_entries":0,"summaries":0,"impact_token":"t"}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::work_org_impact(
                    b,
                    commands::work_view::WorkOrgImpactArgs {
                        task_id: "item:3".into(),
                        org_id: 0,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // ── multi-user M1 (T13): the watcher's pane, and the two reads ──
        //
        // `capture_session` is the WATCHER's view of a live pane, which is
        // the only one sharing gives: `pty_open` would be a direct SSH into
        // the owner's pane that the hub could neither refuse nor revoke. Its
        // two optional arguments are passed as `Some` here on purpose — a
        // mapping that dropped them would still have "worked", with the
        // watcher silently looking at the visible pane and a different cap.
        (
            "capture_session",
            "capture_session",
            json!({ "session_id": 42, "scrollback_lines": 400, "max_lines": 120 }),
            "claude> working on the ticket\n",
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::capture_session(
                    b,
                    commands::sessions::CaptureSessionArgs {
                        session_id: 42,
                        scrollback_lines: Some(400),
                        max_lines: Some(120),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        // Redesign 11.11: the watcher's "Since 13:20" summary; `since`
        // crosses as given.
        (
            "session_summary_since",
            "session_summary_since",
            json!({ "session_id": 42, "since": 1_791_465_600 }),
            r#"{"text":"Fixed it.","check":"passed","since":1791465600,"turns":2,"model":"haiku","host_alias":"h","at":1791466000}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::session_summary_since(
                    b,
                    commands::sessions::SessionSummarySinceArgs {
                        session_id: 42,
                        since: 1_791_465_600,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "session_access",
            "session_access",
            json!({ "session_id": 42 }),
            r#"[{"session_id":42,"person_id":3,"person_name":"jane","level":"watch","granted_by":1,"granted_at":1700000000}]"#,
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::session_access(
                    b,
                    commands::sessions::SessionAccessArgs { session_id: 42 },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // No arguments, and the empty object is the assertion: "whose grants"
        // is the CONNECTION's own person on the hub, never a parameter this
        // side could point at someone else.
        (
            "my_grants",
            "my_grants",
            json!({}),
            r#"{"person_id":1,"grants":[{"session_id":42,"level":"drive"}]}"#,
            Box::new(|b, s, _| block_on(commands::sessions::routed::my_grants(b, s)).map(|_| ())),
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

/// Multi-user M1 (T5): a hub client may not name the OWNER of the session it
/// asks the hub to create.
///
/// `NewSessionArgs::owner_person_id` is `#[serde(skip_deserializing)]` and
/// `HubBackend::new_session` spells its arguments out one by one, so the field
/// has two independent reasons never to cross the wire — and this is the
/// assertion that notices if somebody "fixes" the spelled-out list by
/// serialising the struct instead. Whose a session is follows from the
/// connection the hub authenticated, never from a field in the request; a
/// client that could set it could create a session in a colleague's name.
///
/// The table row above carries `owner_person_id: Some(42)` and asserts the JSON
/// whole, which proves the same thing; this test states it on its own so the
/// failure message names the rule rather than a diff.
#[test]
fn new_session_never_sends_an_owner_over_the_wire() {
    let fake = Fake::answering(SESSION_PAYLOAD);
    let (_dir, st) = store();
    block_on(commands::sessions::routed::new_session(
        &remote_backend(&fake),
        fleet_core::service::sessions::NewSessionArgs {
            host_alias: "trn".into(),
            project_id: 4,
            worktree_id: None,
            name: "demo".into(),
            call_id: None,
            new_worktree: None,
            base_branch: None,
            kind: None,
            start_command: None,
            friendly_name: None,
            resume_claude_session_id: None,
            model: None,
            effort: None,
            profile: None,
            agent: None,
            origin: None,
            over_limit_ok: false,
            owner_person_id: Some(42),
            start_token: None,
        },
        &st,
        &ssh(),
        &fleet_core::cancel::CancellationRegistry::new(),
    ))
    .expect("the hub answers with a row");
    let (tool, args) = fake.only_call();
    assert_eq!(tool, "new_session");
    assert!(
        args.get("owner_person_id").is_none(),
        "the owner must never cross the wire — the hub resolves it from the \
         connection's own person: {args}"
    );
}

/// The table [`every_routed_mutation_names_its_tool_and_arguments`] runs; also run against a configured hub this
/// launch cannot use, which must refuse every row.
fn routed_mutation_cases() -> Vec<Case> {
    let mut cases = routed_mutation_cases_but_the_catalog();
    cases.extend(catalog_admin_cases());
    cases.extend(org_admin_mutation_cases());
    cases
}

/// Org administration phase B: the reads of the hub's `org_admin`.
fn org_admin_read_cases() -> Vec<Case> {
    use fleet_core::service::org_admin::OrgAdminArgs;
    vec![
        (
            "list_devices",
            "org_admin",
            json!({ "action": "list_devices" }),
            r#"[]"#,
            Box::new(|b, s, _| {
                block_on(commands::org_devices::routed::list_devices(
                    b,
                    s,
                    OrgAdminArgs {
                        ..OrgAdminArgs::new("list_devices")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "list_people",
            "org_admin",
            json!({ "action": "list_people" }),
            r#"[]"#,
            Box::new(|b, s, _| {
                block_on(commands::org_devices::routed::list_people(
                    b,
                    s,
                    OrgAdminArgs {
                        ..OrgAdminArgs::new("list_people")
                    },
                ))
                .map(|_| ())
            }),
        ),
    ]
}

/// Org administration phase B: every org, device and people change routes
/// to the hub's `org_admin` under its own action.
fn org_admin_mutation_cases() -> Vec<Case> {
    use fleet_core::service::org_admin::OrgAdminArgs;
    vec![
        (
            "set_org_member",
            "org_admin",
            json!({ "action": "set_member", "org_id": 1, "person": "jane", "role": "admin" }),
            r#"[]"#,
            Box::new(|b, s, _| {
                block_on(commands::orgs::routed::set_org_member(
                    b,
                    s,
                    OrgAdminArgs {
                        org_id: Some(1),
                        person: Some("jane".into()),
                        role: Some("admin".into()),
                        ..OrgAdminArgs::new("set_member")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "remove_org_member",
            "org_admin",
            json!({ "action": "remove_member", "org_id": 1, "person_id": 3 }),
            r#"{"removed":true,"revoked_grants":0}"#,
            Box::new(|b, s, _| {
                block_on(commands::orgs::routed::remove_org_member(
                    b,
                    s,
                    OrgAdminArgs {
                        org_id: Some(1),
                        person_id: Some(3),
                        ..OrgAdminArgs::new("remove_member")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "org_member_grants",
            "org_admin",
            json!({ "action": "member_grants", "org_id": 1, "person_id": 3 }),
            r#"{"watch":4,"drive":2}"#,
            Box::new(|b, s, _| {
                block_on(commands::orgs::routed::org_member_grants(
                    b,
                    s,
                    OrgAdminArgs {
                        org_id: Some(1),
                        person_id: Some(3),
                        ..OrgAdminArgs::new("member_grants")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "set_org_setting",
            "org_admin",
            json!({ "action": "set_org_setting", "org_id": 1, "key": "budget.org_daily_usd", "value": "5" }),
            r#"[]"#,
            Box::new(|b, s, _| {
                block_on(commands::orgs::routed::set_org_setting(
                    b,
                    s,
                    OrgAdminArgs {
                        org_id: Some(1),
                        key: Some("budget.org_daily_usd".into()),
                        value: Some("5".into()),
                        ..OrgAdminArgs::new("set_org_setting")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "add_org",
            "org_admin",
            json!({ "action": "add_org", "name": "Acme" }),
            r#"{"id":1}"#,
            Box::new(|b, s, _| {
                block_on(commands::orgs::routed::add_org(
                    b,
                    s,
                    OrgAdminArgs {
                        name: Some("Acme".into()),
                        ..OrgAdminArgs::new("add_org")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "update_org",
            "org_admin",
            json!({ "action": "update_org", "org_id": 1, "jev": "on" }),
            r#"{"id":1}"#,
            Box::new(|b, s, _| {
                block_on(commands::orgs::routed::update_org(
                    b,
                    s,
                    OrgAdminArgs {
                        org_id: Some(1),
                        jev: Some("on".into()),
                        ..OrgAdminArgs::new("update_org")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "remove_org",
            "org_admin",
            json!({ "action": "remove_org", "org_id": 1 }),
            r#"{"removed":1}"#,
            Box::new(|b, s, _| {
                block_on(commands::orgs::routed::remove_org(
                    b,
                    s,
                    OrgAdminArgs {
                        org_id: Some(1),
                        ..OrgAdminArgs::new("remove_org")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "add_org_rule",
            "org_admin",
            json!({ "action": "add_rule", "org_id": 1, "owner": "acme" }),
            r#"{"id":3}"#,
            Box::new(|b, s, _| {
                block_on(commands::orgs::routed::add_org_rule(
                    b,
                    s,
                    OrgAdminArgs {
                        org_id: Some(1),
                        owner: Some("acme".into()),
                        ..OrgAdminArgs::new("add_rule")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "remove_org_rule",
            "org_admin",
            json!({ "action": "remove_rule", "rule_id": 3 }),
            r#"{"removed":3}"#,
            Box::new(|b, s, _| {
                block_on(commands::orgs::routed::remove_org_rule(
                    b,
                    s,
                    OrgAdminArgs {
                        rule_id: Some(3),
                        ..OrgAdminArgs::new("remove_rule")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "assign_host_org",
            "org_admin",
            json!({ "action": "assign_host", "host_alias": "h", "org_id": 1 }),
            r#"{}"#,
            Box::new(|b, s, _| {
                block_on(commands::orgs::routed::assign_host_org(
                    b,
                    s,
                    OrgAdminArgs {
                        host_alias: Some("h".into()),
                        org_id: Some(1),
                        ..OrgAdminArgs::new("assign_host")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "assign_tracker_org",
            "org_admin",
            json!({ "action": "assign_tracker", "tracker_id": 2, "org_id": 1 }),
            r#"{}"#,
            Box::new(|b, s, _| {
                block_on(commands::orgs::routed::assign_tracker_org(
                    b,
                    s,
                    OrgAdminArgs {
                        tracker_id: Some(2),
                        org_id: Some(1),
                        ..OrgAdminArgs::new("assign_tracker")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "pair_device",
            "org_admin",
            json!({ "action": "pair_device", "device": "phone", "mode": "readonly" }),
            r#"{}"#,
            Box::new(|b, s, _| {
                block_on(commands::org_devices::routed::pair_device(
                    b,
                    s,
                    OrgAdminArgs {
                        device: Some("phone".into()),
                        mode: Some("readonly".into()),
                        ..OrgAdminArgs::new("pair_device")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "revoke_device",
            "org_admin",
            json!({ "action": "revoke_device", "device": "phone" }),
            r#"{"revoked":"phone"}"#,
            Box::new(|b, s, _| {
                block_on(commands::org_devices::routed::revoke_device(
                    b,
                    s,
                    OrgAdminArgs {
                        device: Some("phone".into()),
                        ..OrgAdminArgs::new("revoke_device")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "set_device_trust",
            "org_admin",
            json!({ "action": "set_device_trust", "device": "phone", "trusted": true }),
            r#"{}"#,
            Box::new(|b, s, _| {
                block_on(commands::org_devices::routed::set_device_trust(
                    b,
                    s,
                    OrgAdminArgs {
                        device: Some("phone".into()),
                        trusted: Some(true),
                        ..OrgAdminArgs::new("set_device_trust")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "update_device",
            "org_admin",
            json!({ "action": "rename_device", "device": "phone", "name": "Ada's phone" }),
            r#"{}"#,
            Box::new(|b, s, _| {
                block_on(commands::org_devices::routed::update_device(
                    b,
                    s,
                    commands::org_devices::UpdateDeviceArgs {
                        device: "phone".into(),
                        name: Some("Ada's phone".into()),
                        ..Default::default()
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "bind_device_org",
            "org_admin",
            json!({ "action": "bind_device", "device": "phone", "org": "Acme" }),
            r#"{}"#,
            Box::new(|b, s, _| {
                block_on(commands::org_devices::routed::bind_device_org(
                    b,
                    s,
                    OrgAdminArgs {
                        device: Some("phone".into()),
                        org: Some("Acme".into()),
                        ..OrgAdminArgs::new("bind_device")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "set_device_person",
            "org_admin",
            json!({ "action": "set_device_person", "device": "phone", "person": "ada" }),
            r#"{}"#,
            Box::new(|b, s, _| {
                block_on(commands::org_devices::routed::set_device_person(
                    b,
                    s,
                    OrgAdminArgs {
                        device: Some("phone".into()),
                        person: Some("ada".into()),
                        ..OrgAdminArgs::new("set_device_person")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "grant_device_catalog",
            "org_admin",
            json!({ "action": "grant_catalog", "device": "phone", "catalog": "personal", "on": true }),
            r#"{}"#,
            Box::new(|b, s, _| {
                block_on(commands::org_devices::routed::grant_device_catalog(
                    b,
                    s,
                    OrgAdminArgs {
                        device: Some("phone".into()),
                        catalog: Some("personal".into()),
                        on: Some(true),
                        ..OrgAdminArgs::new("grant_catalog")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "rename_person",
            "org_admin",
            json!({ "action": "rename_person", "person_id": 2, "name": "ada" }),
            r#"{}"#,
            Box::new(|b, s, _| {
                block_on(commands::org_devices::routed::rename_person(
                    b,
                    s,
                    OrgAdminArgs {
                        person_id: Some(2),
                        name: Some("ada".into()),
                        ..OrgAdminArgs::new("rename_person")
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "disable_person",
            "org_admin",
            json!({ "action": "disable_person", "person_id": 2 }),
            r#"{}"#,
            Box::new(|b, s, _| {
                block_on(commands::org_devices::routed::disable_person(
                    b,
                    s,
                    OrgAdminArgs {
                        person_id: Some(2),
                        ..OrgAdminArgs::new("disable_person")
                    },
                ))
                .map(|_| ())
            }),
        ),
    ]
}

/// The mutations outside the asset catalog; [`catalog_admin_cases`] is the rest.
fn routed_mutation_cases_but_the_catalog() -> Vec<Case> {
    use commands::sessions::RepairSessionArgs;
    use fleet_core::service::add_project::{AddProjectArgs, AddProjectSource};
    use fleet_core::service::bg_sessions::NewBgSessionArgs;
    use fleet_core::service::hosts::HostAliasArgs;
    use fleet_core::service::move_session::resolve::{ResolveMoveAction, ResolveMoveArgs};
    use fleet_core::service::move_session::MoveSessionArgs;
    use fleet_core::service::rewind::{RewindArgs, RewindMode};
    use fleet_core::service::safe_kill::SafeKillSessionArgs;
    use fleet_core::service::sessions::{
        AdoptSessionArgs, DiscoverLostSessionsArgs, DismissGhostSessionArgs, KillSessionArgs,
        LostTargetArgs, NewSessionArgs, PlaceTranscriptArgs, RecreateSessionArgs,
        RenameSessionArgs, RestartSessionArgs, RestoreHostSessionsArgs, SendPromptArgs,
        SetFriendlyNameArgs, SetSessionTagsArgs, SpawnReviewArgs, TouchSessionViewedArgs,
    };

    vec![
        (
            "link_peer_hub",
            "link_peer",
            json!({ "url": "https://hub.other.example", "code": "AB12-CD34" }),
            "null",
            Box::new(|b, _, _| {
                block_on(commands::federation::routed::link_peer_hub(
                    b,
                    commands::federation::LinkPeerHubArgs {
                        url: "https://hub.other.example".into(),
                        code: "AB12-CD34".into(),
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "unlink_peer_hub",
            "unlink_peer",
            json!({ "id": 1 }),
            r#"{"id":1,"failed_messages":0}"#,
            Box::new(|b, _, _| {
                block_on(commands::federation::routed::unlink_peer_hub(
                    b,
                    commands::federation::UnlinkPeerHubArgs { id: 1 },
                ))
                .map(|_| ())
            }),
        ),
        // Declarative pages P6: a write the hub records as this device.
        (
            "set_fleet_setting",
            "set_setting",
            json!({ "key": "work.recent_days", "value": "3" }),
            r#"{"work.recent_days":"3"}"#,
            Box::new(|b, s, _| {
                block_on(commands::pages::routed::set_fleet_setting(
                    b,
                    s,
                    "work.recent_days".into(),
                    "3".into(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "decide_guide",
            "guide",
            json!({ "action": "decide", "id": 7, "approve": true }),
            r#"{"guides":[],"proposals":[],"can_write":true}"#,
            Box::new(|b, s, _| {
                block_on(commands::pages::routed::decide_guide(b, s, 7, true)).map(|_| ())
            }),
        ),
        (
            "remove_guide",
            "guide",
            json!({ "action": "remove", "page_id": "guide.cleanup" }),
            r#"{"guides":[],"proposals":[],"can_write":true}"#,
            Box::new(|b, s, _| {
                block_on(commands::pages::routed::remove_guide(
                    b,
                    s,
                    "guide.cleanup".into(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "answer_form",
            "ask",
            json!({ "answer": "f_a", "values": { "x": "y" } }),
            FORM_VIEW_JSON,
            Box::new(|b, s, ssh| {
                let values = serde_json::from_value(json!({ "x": "y" })).unwrap();
                block_on(commands::forms::routed::answer_form(
                    b,
                    s,
                    ssh,
                    "f_a".into(),
                    values,
                ))
                .map(|_| ())
            }),
        ),
        (
            "scan_debug_devices",
            "debug_devices",
            json!({ "action": "scan", "host": "mac" }),
            r#"{"hosts":[]}"#,
            Box::new(|b, s, ssh| {
                block_on(commands::debug_devices::routed::scan_debug_devices(
                    b,
                    s,
                    ssh,
                    commands::debug_devices::ScanDebugDevicesArgs {
                        host: Some("mac".into()),
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "update_debug_device",
            "debug_devices",
            json!({ "action": "configure", "device": "3", "label": "bench", "shared": true }),
            DEBUG_DEVICE_JSON,
            Box::new(|b, s, _| {
                block_on(commands::debug_devices::routed::update_debug_device(
                    b,
                    s,
                    commands::debug_devices::UpdateDebugDeviceArgs {
                        id: 3,
                        label: Some("bench".into()),
                        shared: Some(true),
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "release_debug_device",
            "debug_devices",
            json!({ "action": "release", "device": "3" }),
            DEBUG_DEVICE_JSON,
            Box::new(|b, s, _| {
                block_on(commands::debug_devices::routed::release_debug_device(
                    b,
                    s,
                    commands::debug_devices::DebugDeviceArgs { id: 3 },
                ))
                .map(|_| ())
            }),
        ),
        (
            "forget_debug_device",
            "debug_devices",
            json!({ "action": "forget", "device": "3" }),
            r#"{"removed":true}"#,
            Box::new(|b, s, _| {
                block_on(commands::debug_devices::routed::forget_debug_device(
                    b,
                    s,
                    commands::debug_devices::DebugDeviceArgs { id: 3 },
                ))
                .map(|_| ())
            }),
        ),
        (
            "boot_debug_device",
            "debug_devices",
            json!({ "action": "boot", "device": "3" }),
            r#"{"state":"booted"}"#,
            Box::new(|b, s, ssh| {
                block_on(commands::debug_devices::routed::boot_debug_device(
                    b,
                    s,
                    ssh,
                    commands::debug_devices::DebugDeviceArgs { id: 3 },
                ))
                .map(|_| ())
            }),
        ),
        (
            "claim_debug_device",
            "debug_devices",
            json!({ "action": "claim", "device": "3", "note": "login flow", "claim_s": null }),
            DEBUG_DEVICE_JSON,
            Box::new(|b, s, _| {
                block_on(commands::debug_devices::routed::claim_debug_device(
                    b,
                    s,
                    commands::debug_devices::ClaimDebugDeviceArgs {
                        id: 3,
                        note: Some("login flow".into()),
                        claim_s: None,
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "install_debug_device",
            "debug_devices",
            json!({ "action": "install", "device": "3", "path": "~/app.apk", "host": null }),
            RUN_OUTPUT_JSON,
            Box::new(|b, s, ssh| {
                block_on(commands::debug_devices::routed::install_debug_device(
                    b,
                    s,
                    ssh,
                    commands::debug_devices::InstallDebugDeviceArgs {
                        id: 3,
                        path: "~/app.apk".into(),
                        // An empty host means the device's own: sent as absent.
                        host: Some(" ".into()),
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "debug_device_logs",
            "debug_devices",
            json!({ "action": "logs", "device": "3", "lines": 200, "contains": "FATAL" }),
            RUN_OUTPUT_JSON,
            Box::new(|b, s, ssh| {
                block_on(commands::debug_devices::routed::debug_device_logs(
                    b,
                    s,
                    ssh,
                    commands::debug_devices::DebugDeviceLogsArgs {
                        id: 3,
                        contains: Some("FATAL".into()),
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "shutdown_debug_device",
            "debug_devices",
            json!({ "action": "shutdown", "device": "3" }),
            DEBUG_DEVICE_JSON,
            Box::new(|b, s, ssh| {
                block_on(commands::debug_devices::routed::shutdown_debug_device(
                    b,
                    s,
                    ssh,
                    commands::debug_devices::DebugDeviceArgs { id: 3 },
                ))
                .map(|_| ())
            }),
        ),
        (
            "decline_form",
            "ask",
            json!({ "decline": "f_a", "note": "later" }),
            FORM_VIEW_JSON,
            Box::new(|b, s, _| {
                block_on(commands::forms::routed::decline_form(
                    b,
                    s,
                    "f_a".into(),
                    Some("later".into()),
                ))
                .map(|_| ())
            }),
        ),
        (
            "control_route_propose",
            "control_route",
            json!({ "action": "propose", "text": "how is the federation handshake doing" }),
            r#"{"outcome":"none","targets":[]}"#,
            Box::new(|b, s, _| {
                block_on(commands::operator::routed::control_route_propose(
                    b,
                    s,
                    "how is the federation handshake doing".into(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "control_route_follow",
            "control_route",
            json!({ "action": "follow", "run_id": 7, "chosen": "m3" }),
            "true",
            Box::new(|b, s, _| {
                block_on(commands::operator::routed::control_route_follow(
                    b,
                    s,
                    7,
                    "m3".into(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "mcp_confirm",
            "answer_mcp_confirm",
            json!({ "nonce": "n", "approved": true }),
            "true",
            Box::new(|b, _, _| {
                let guards = fleet_core::mcp::McpGuards::new(Arc::new(|_| {}));
                block_on(commands::mcp::routed::mcp_confirm(
                    b,
                    &guards,
                    "n".into(),
                    true,
                ))
                .map(|_| ())
            }),
        ),
        (
            "decide_setting_proposals",
            "decide_setting_proposals",
            json!({ "accept": [4], "reject": [5] }),
            r#"{"applied":[4],"rejected":[5],"failed":[]}"#,
            Box::new(|b, s, _| {
                block_on(commands::pages::routed::decide_setting_proposals(
                    b,
                    s,
                    vec![4],
                    vec![5],
                ))
                .map(|_| ())
            }),
        ),
        (
            "send_prompt",
            "send_prompt",
            // `prompt` must be empty alongside `keys` (the hub refuses text
            // and a key press together, same as the local path) — this row
            // still proves `keys` crosses the wire.
            json!({ "host_alias": "trn", "tmux_name": "demo", "prompt": "", "submit": true, "keys": "Enter" }),
            r#"{"delivered":true}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::send_prompt(
                    b,
                    SendPromptArgs {
                        host_alias: "trn".into(),
                        tmux_name: "demo".into(),
                        prompt: "".into(),
                        submit: true,
                        keys: Some("Enter".into()),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "kill_session",
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
            "shell_terminals",
            "shell_terminals",
            json!({ "session_id": 7, "action": "open", "n": 2, "at": "home" }),
            r#"{"session_id":7,"host_alias":"trn","terminals":[{"n":2,"tmux_name":"demo--sh2"}],"opened":2}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::shell_terminals(
                    b,
                    fleet_core::service::sessions::ShellTerminalsArgs {
                        session_id: 7,
                        action: fleet_core::service::sessions::ShellTerminalAction::Open,
                        n: Some(2),
                        at: fleet_core::service::sessions::ShellTerminalStart::Home,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "safe_kill_session",
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
            "set_session_friendly_name",
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
            "set_session_tags",
            "set_session_tags",
            json!({ "session_id": 7, "tags": ["release"] }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::set_session_tags(
                    b,
                    SetSessionTagsArgs {
                        session_id: 7,
                        tags: vec!["release".into()],
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "decide_related_session",
            "decide_related_session",
            json!({ "session_id": 7, "run_id": 3, "linked": true }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::decide_related_session(
                    b,
                    fleet_core::service::decide::related_session::DecideRelatedSessionArgs {
                        session_id: 7,
                        run_id: 3,
                        linked: true,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "touch_session_viewed",
            "touch_session_viewed",
            json!({ "session_id": 7 }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::touch_session_viewed(
                    b,
                    TouchSessionViewedArgs { session_id: 7 },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "link_session_work",
            "work_link",
            // A person on the desktop: the source is always `manual`.
            json!({ "session_id": 7, "action": "link", "key": "ABC-1", "item_id": null,
                    "link_id": null, "source": "manual" }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::link_session_work(
                    b,
                    commands::work::LinkSessionWorkArgs {
                        session_id: 7,
                        key: Some("ABC-1".into()),
                        item_id: None,
                        force_cross_org: false,
                        ..Default::default()
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "start_work",
            "work_link",
            // Only the start fields travel.
            json!({ "session_id": null, "action": "start", "key": "ABC-1", "item_id": null,
                    "link_id": null, "source": null, "project_id": 3, "host_alias": "h",
                    "with_brief": true }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::trackers::routed::start_work(
                    b,
                    commands::trackers::StartWorkArgs {
                        reference: Some("ABC-1".into()),
                        project_id: Some(3),
                        host_alias: Some("h".into()),
                        with_brief: true,
                        ..Default::default()
                    },
                    s,
                    &ssh(),
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "preview_start_work",
            "work_link",
            // The start's own fields, under its own action.
            json!({ "session_id": null, "action": "preview_start", "key": "ABC-1", "item_id": null,
                    "link_id": null, "source": null, "project_id": 3, "host_alias": "h",
                    "with_brief": true, "parallel": true }),
            r#"{"key":"ABC-1","title":"","item_id":null,"plan":null,"missing":"host","projects":[],"hosts":[],"conflicts":[],"brief":null,"checkout":null}"#,
            Box::new(|b, s, _| {
                block_on(commands::trackers::routed::preview_start_work(
                    b,
                    commands::trackers::StartWorkArgs {
                        reference: Some("ABC-1".into()),
                        project_id: Some(3),
                        host_alias: Some("h".into()),
                        with_brief: true,
                        parallel: true,
                        ..Default::default()
                    },
                    s,
                    &ssh(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "abandon_start",
            "work_link",
            json!({ "session_id": 7, "action": "abandon_start", "key": null, "item_id": null,
                    "link_id": null, "source": null }),
            r#"{"session_id":7,"worktree_removed":true,"branch_deleted":true}"#,
            Box::new(|b, s, ssh| {
                block_on(commands::trackers::routed::abandon_start(
                    b,
                    commands::trackers::AbandonStartArgs { session_id: 7 },
                    s,
                    ssh,
                ))
                .map(|_| ())
            }),
        ),
        (
            "start_work_multi",
            "work_link",
            json!({ "session_id": null, "action": "start", "key": "ABC-1", "item_id": null,
                    "link_id": null, "source": null, "project_ids": [3, 4], "host_alias": "h",
                    "with_brief": true }),
            r#"{"key":"ABC-1","started":[]}"#,
            Box::new(|b, s, _| {
                block_on(commands::trackers::routed::start_work_multi(
                    b,
                    commands::trackers::StartWorkMultiArgs {
                        start: commands::trackers::StartWorkArgs {
                            reference: Some("ABC-1".into()),
                            host_alias: Some("h".into()),
                            with_brief: true,
                            ..Default::default()
                        },
                        project_ids: vec![3, 4],
                    },
                    s,
                    &ssh(),
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "request_work_handover",
            "work_link",
            json!({ "session_id": 5, "action": "handover", "key": null, "item_id": null,
                    "link_id": null, "source": null }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::request_work_handover(
                    b,
                    commands::work::RequestWorkHandoverArgs { session_id: 5 },
                    s,
                    &ssh(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "summarize_past_work",
            "work_link",
            json!({ "session_id": null, "action": "summarize", "key": "ABC-1", "item_id": null,
                    "link_id": 4, "source": null }),
            SUMMARY_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::summarize_past_work(
                    b,
                    commands::work::SummarizePastWorkArgs {
                        key: "ABC-1".into(),
                        link_id: 4,
                    },
                    s,
                    &ssh(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "resume_work",
            "work_link",
            // Only the resume fields travel; an older hub never sees them
            // for the other work_link commands.
            json!({ "session_id": null, "action": "resume", "key": "ABC-1", "item_id": null,
                    "link_id": 4, "source": null, "mode": "brief", "brief": "edited" }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::resume_work(
                    b,
                    commands::work::ResumeWorkArgs {
                        key: "ABC-1".into(),
                        mode: "brief".into(),
                        link_id: Some(4),
                        host_alias: None,
                        brief: Some("edited".into()),
                        force_cross_org: false,
                    },
                    s,
                    &ssh(),
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "reject_session_work",
            "work_link",
            json!({ "session_id": 7, "action": "reject", "key": null, "item_id": 3,
                    "link_id": null, "source": null }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::reject_session_work(
                    b,
                    commands::work::RejectSessionWorkArgs {
                        session_id: 7,
                        key: None,
                        item_id: Some(3),
                        link_id: None,
                        ..Default::default()
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "unlink_session_work",
            "work_link",
            json!({ "session_id": 7, "action": "unlink", "key": null, "item_id": null,
                    "link_id": 5, "source": null }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::unlink_session_work(
                    b,
                    commands::work::UnlinkSessionWorkArgs {
                        session_id: 7,
                        link_id: 5,
                        ..Default::default()
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "confirm_session_work",
            "work_link",
            json!({ "session_id": 7, "action": "confirm", "key": null, "item_id": null,
                    "link_id": 5, "source": null }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::confirm_session_work(
                    b,
                    commands::work::ConfirmSessionWorkArgs {
                        session_id: 7,
                        link_id: 5,
                        force_cross_org: false,
                        ..Default::default()
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // Work graph M14.1d: a secondary link and the compare-and-set guard
        // travel on the existing decisions.
        (
            "link_session_work",
            "work_link",
            json!({ "session_id": 7, "action": "link", "key": "ABC-1", "item_id": null,
                    "link_id": null, "source": "manual", "primary": false,
                    "expected_version": 2 }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::link_session_work(
                    b,
                    commands::work::LinkSessionWorkArgs {
                        session_id: 7,
                        key: Some("ABC-1".into()),
                        primary: Some(false),
                        expected_version: Some(2),
                        ..Default::default()
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "confirm_session_work",
            "work_link",
            json!({ "session_id": 7, "action": "confirm", "key": null, "item_id": null,
                    "link_id": 5, "source": null, "primary": false, "expected_version": 3 }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::confirm_session_work(
                    b,
                    commands::work::ConfirmSessionWorkArgs {
                        session_id: 7,
                        link_id: 5,
                        primary: Some(false),
                        expected_version: Some(3),
                        ..Default::default()
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "reject_session_work",
            "work_link",
            json!({ "session_id": 7, "action": "reject", "key": null, "item_id": null,
                    "link_id": 5, "source": null, "expected_version": 3 }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::reject_session_work(
                    b,
                    commands::work::RejectSessionWorkArgs {
                        session_id: 7,
                        link_id: Some(5),
                        expected_version: Some(3),
                        ..Default::default()
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "unlink_session_work",
            "work_link",
            json!({ "session_id": 7, "action": "unlink", "key": null, "item_id": null,
                    "link_id": 5, "source": null, "expected_version": 4 }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::unlink_session_work(
                    b,
                    commands::work::UnlinkSessionWorkArgs {
                        session_id: 7,
                        link_id: 5,
                        expected_version: Some(4),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "unarchive_session_work",
            "work_link",
            json!({ "session_id": 7, "action": "unarchive", "key": null, "item_id": null,
                    "link_id": null, "source": null }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::unarchive_session_work(
                    b,
                    commands::work::SessionLifecycleArgs { session_id: 7 },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "tidy_apply",
            "work_link",
            json!({ "session_id": null, "action": "tidy_apply", "key": null, "item_id": null,
                    "link_id": null, "source": null,
                    "items": [{ "session_id": 7, "action": "safe_kill" }] }),
            r#"{"results":[{"session_id":7,"action":"safe_kill","ok":true,"outcome":"safe_kill_requested"}]}"#,
            Box::new(|b, s, ssh| {
                block_on(commands::work::routed::tidy_apply(
                    b,
                    commands::work::TidyApplyArgs {
                        items: vec![fleet_core::service::work::tidy::TidyApplyItem {
                            session_id: 7,
                            action: "safe_kill".into(),
                            ..Default::default()
                        }],
                    },
                    s,
                    ssh,
                ))
                .map(|_| ())
            }),
        ),
        (
            "dismiss_reopened",
            "work_link",
            json!({ "session_id": null, "action": "dismiss", "key": null, "item_id": 3,
                    "link_id": null, "source": null }),
            r#"{"dismissed":true}"#,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::dismiss_reopened(
                    b,
                    commands::work::DismissReopenedArgs { item_id: 3 },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "set_work_project_trust",
            "work_link",
            json!({ "session_id": null, "action": "trust_project", "key": null,
                    "item_id": null, "link_id": null, "source": null,
                    "project_id": 3, "on": true }),
            r#"{"trusted":[3]}"#,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::set_work_project_trust(
                    b,
                    commands::work::SetWorkProjectTrustArgs {
                        project_id: 3,
                        on: true,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "restart_session",
            "restart_session",
            json!({ "host_alias": "trn", "name": "demo", "force": false, "profile": "work" }),
            SESSION_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::restart_session(
                    b,
                    RestartSessionArgs {
                        host_alias: "trn".into(),
                        name: "demo".into(),
                        force: false,
                        profile: Some("work".into()),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "rewind_conversation",
            "rewind_conversation",
            json!({
                "session_id": 7,
                "anchor_uuid": "aaaaaaaa-0000-0000-0000-000000000002",
                "mode": "fork",
                "new_worktree": null,
            }),
            SESSION_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::rewind_conversation(
                    b,
                    RewindArgs {
                        session_id: 7,
                        anchor_uuid: Some("aaaaaaaa-0000-0000-0000-000000000002".into()),
                        mode: RewindMode::Fork,
                        new_worktree: None,
                    },
                    s,
                    h,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "spawn_review",
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
                        origin: None,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        // `call_id` is this process's own cancellation-registry key and has no
        // hub counterpart, so the tool must never see it — not as a value and
        // not as a `null`. The case above passes `None`, which proves nothing
        // about a key that would only appear when it is set; this one sets it.
        (
            "spawn_review",
            "spawn_review",
            json!({ "source_session_id": 7, "prompt": "review it" }),
            SESSION_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::spawn_review(
                    b,
                    SpawnReviewArgs {
                        source_session_id: 7,
                        prompt: "review it".into(),
                        call_id: Some(123),
                        origin: None,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "queue_prompt",
            "queue_prompt",
            json!({ "session_id": 7, "prompt": "rebase on main" }),
            r#"{"session_id":7,"delivered":false,"queued_id":3}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::queue_prompt(
                    b,
                    fleet_core::service::sessions::QueuePromptArgs {
                        session_id: 7,
                        prompt: "rebase on main".into(),
                        ..Default::default()
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        // Taking a prompt back is the same tool with `cancel` set.
        (
            "cancel_queued_prompt",
            "queued_prompts",
            json!({ "session_id": 7, "cancel": 3 }),
            "[]",
            Box::new(|b, s, _h| {
                block_on(commands::sessions::routed::cancel_queued_prompt(
                    b,
                    fleet_core::service::sessions::CancelQueuedPromptArgs {
                        session_id: 7,
                        id: 3,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "recreate_session",
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
            "restore_host_sessions",
            "restore_host_sessions",
            json!({ "host_alias": "trn", "dry_run": true, "session_ids": [7, 9] }),
            r#"{"host_alias":"trn","dry_run":true,"plan":[],"results":[]}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::restore_host_sessions(
                    b,
                    RestoreHostSessionsArgs {
                        host_alias: "trn".into(),
                        dry_run: true,
                        session_ids: Some(vec![7, 9]),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "discover_lost_sessions",
            "discover_lost_sessions",
            json!({ "host_alias": "trn", "limit": 25 }),
            "[]",
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::discover_lost_sessions(
                    b,
                    DiscoverLostSessionsArgs {
                        host_alias: "trn".into(),
                        limit: Some(25),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "lost_target",
            "lost_target",
            json!({ "session_id": 7 }),
            "{}",
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::lost_target(
                    b,
                    LostTargetArgs {
                        session_id: Some(7),
                        ..Default::default()
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "place_transcript",
            "place_transcript",
            json!({
                "host_alias": "trn",
                "claude_session_id": "44366faf-ae97-426a-91cd-beaf3c74f1d7",
                "project_id": 3,
            }),
            r#"{"project_id":3,"tmux_name":"dev-o-r","copied":true}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::place_transcript(
                    b,
                    PlaceTranscriptArgs {
                        host_alias: "trn".into(),
                        claude_session_id: "44366faf-ae97-426a-91cd-beaf3c74f1d7".into(),
                        project_id: 3,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "dismiss_ghost_session",
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
            "adopt_session",
            "adopt_session",
            json!({ "session_id": 7 }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::adopt_session(
                    b,
                    AdoptSessionArgs {
                        session_id: 7,
                        project_id: None,
                        owner_person_id: Some(1),
                        decider: Default::default(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "new_bg_session",
            "new_bg_session",
            json!({ "host_alias": "trn", "name": "worker", "prompt": "go", "requester_session_id": 41 }),
            r#"{"claude_session_id":"abc"}"#,
            Box::new(|b, s, h| {
                block_on(commands::sessions::routed::new_bg_session(
                    b,
                    NewBgSessionArgs {
                        host_alias: "trn".into(),
                        name: "worker".into(),
                        prompt: "go".into(),
                        requester_session_id: Some(41),
                        ..Default::default()
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "cancel_task",
            "cancel_task",
            json!({ "task_id": 11 }),
            TASK_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::tasks::routed::cancel_task(b, 11, s)).map(|_| ())
            }),
        ),
        (
            "install_agent",
            "install_agent",
            json!({ "alias": "trn", "hub_url": null, "version": null }),
            r#"{"id":1,"host_alias":"trn","version":"0.6.0","state":"running","step":"target","started_at":1}"#,
            Box::new(|b, s, h| {
                block_on(commands::hosts::routed::install_agent(
                    b,
                    fleet_core::service::agent_install::InstallAgentArgs {
                        alias: "trn".into(),
                        hub_url: None,
                        version: None,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "probe_host",
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
            "move_session",
            json!({ "session_id": 7, "target_host_alias": "hetzner", "keep_source": false, "strict": true, "clean_target": false, "dry_run": true, "when": "now" }),
            MOVE_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::move_session::routed::move_session(
                    b,
                    MoveSessionArgs {
                        session_id: 7,
                        target_host_alias: "hetzner".into(),
                        keep_source: false,
                        strict: true,
                        clean_target: false,
                        dry_run: true,
                        when: fleet_core::service::move_session::When::Now,
                        force_cross_org: false,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        // The other side of both move flags. `strict` in particular decides
        // whether a dirty worktree or an unpushed branch is refused or
        // carried, so "the desktop sent the flag the user chose" is pinned
        // for each value rather than for one of them.
        (
            "move_session",
            "move_session",
            json!({ "session_id": 7, "target_host_alias": "hetzner", "keep_source": true, "strict": false, "clean_target": true, "dry_run": false, "when": "now" }),
            MOVE_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::move_session::routed::move_session(
                    b,
                    MoveSessionArgs {
                        session_id: 7,
                        target_host_alias: "hetzner".into(),
                        keep_source: true,
                        strict: false,
                        clean_target: true,
                        dry_run: false,
                        when: fleet_core::service::move_session::When::Now,
                        force_cross_org: false,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        // A non-default `when`: Transfer 3c Task 4's own lesson (3d shipped
        // a routed argument that reached the schema and the service but was
        // never mapped into the wire args a hub actually receives) — pin
        // that `when` itself, not just `dry_run`/`clean_target`, survives
        // the trip onto the wire.
        (
            "move_session",
            "move_session",
            json!({ "session_id": 7, "target_host_alias": "hetzner", "keep_source": false, "strict": false, "clean_target": false, "dry_run": false, "when": "idle" }),
            MOVE_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::move_session::routed::move_session(
                    b,
                    MoveSessionArgs {
                        session_id: 7,
                        target_host_alias: "hetzner".into(),
                        keep_source: false,
                        strict: false,
                        clean_target: false,
                        dry_run: false,
                        when: fleet_core::service::move_session::When::Idle,
                        force_cross_org: false,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        // `force_cross_org` (work graph M5): off the wire when false, so
        // the three cases above reach an older hub exactly as before, and
        // pinned here when true, so the one flag that turns a refused
        // cross-org move into a carried one cannot stop short of the wire.
        (
            "move_session",
            "move_session",
            json!({ "session_id": 7, "target_host_alias": "hetzner", "keep_source": false, "strict": false, "clean_target": false, "dry_run": false, "when": "now", "force_cross_org": true }),
            MOVE_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::move_session::routed::move_session(
                    b,
                    MoveSessionArgs {
                        session_id: 7,
                        target_host_alias: "hetzner".into(),
                        keep_source: false,
                        strict: false,
                        clean_target: false,
                        dry_run: false,
                        when: fleet_core::service::move_session::When::Now,
                        force_cross_org: true,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        // The target session of a resolved partial move — the id a
        // `session_move_partial` timeline event names, not the source's.
        (
            "resolve_move",
            "resolve_move",
            json!({ "session_id": 43, "action": "finish" }),
            RESOLVE_MOVE_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::resolve_move::routed::resolve_move(
                    b,
                    ResolveMoveArgs {
                        session_id: 43,
                        action: ResolveMoveAction::Finish,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        // #146: `kind`, `start_command` and `friendly_name` now map
        // one-to-one onto the tool's `NewSessionParams`, so `new_session`
        // routes unconditionally (`call_id` is this process's own
        // cancellation-registry key and has no counterpart — never sent).
        (
            // The `new` source with every field set proves the tagged enum
            // crosses the wire; `call_id: Some(123)` proves it does not.
            "add_project",
            "add_project",
            json!({
                "host_alias": "trn",
                "source": {
                    "kind": "new",
                    "owner": "o",
                    "repo": "r",
                    "create_remote": true,
                    "confirm": "tok"
                }
            }),
            PROJECT_TREE_PAYLOAD,
            Box::new(|b, s, h| {
                block_on(commands::projects::routed::add_project(
                    b,
                    AddProjectArgs {
                        host_alias: "trn".into(),
                        source: AddProjectSource::New {
                            owner: "o".into(),
                            repo: "r".into(),
                            create_remote: true,
                            confirm: Some("tok".into()),
                        },
                        call_id: Some(123),
                    },
                    s,
                    h,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            // Every field non-default, so the whole struct is proven to cross the wire.
            "set_project_pick",
            "set_project_pick",
            json!({ "owner": "o", "repo": "r", "pinned": true, "vis": "hide", "grp": "tools" }),
            r#"{"owner":"o","repo":"r","pinned":true,"vis":"hide","grp":"tools"}"#,
            Box::new(|b, s, _| {
                block_on(commands::projects::routed::set_project_pick(
                    b,
                    s,
                    fleet_core::service::project_picks::SetProjectPickArgs {
                        owner: "o".into(),
                        repo: "r".into(),
                        pinned: true,
                        vis: Some("hide".into()),
                        grp: Some("tools".into()),
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "new_session",
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
                "resume_claude_session_id": "550e8400-e29b-41d4-a716-446655440000",
                "model": "opus",
                "effort": "high",
                "profile": "work",
                "agent": "claude",
                "start_token": "st-demo-1",
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
                        resume_claude_session_id: Some(
                            "550e8400-e29b-41d4-a716-446655440000".into(),
                        ),
                        model: Some("opus".into()),
                        effort: Some("high".into()),
                        profile: Some("work".into()),
                        agent: Some("claude".into()),
                        origin: None,
                        // Set, and absent from the asserted JSON above: whose
                        // a session is follows from the CONNECTION, never from
                        // an argument a client could choose (multi-user M1,
                        // T5). `new_session_never_sends_an_owner_over_the_wire`
                        // says it in one assertion as well.
                        over_limit_ok: false,
                        owner_person_id: Some(42),
                        start_token: Some("st-demo-1".into()),
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
        // `ensure_operator` needs `ssh` and `reg` the way `move_session` and
        // `new_session` do, so it copies their entry shape rather than the
        // simpler `(b, s, _)` one `operator_status` below uses. Its service
        // call additionally needs the store as an `Arc` (to clone into the
        // `LiveHost` it hands to `new_session`'s own lifecycle, which
        // outlives a single lock), which the table's shared `Mutex<Store>`
        // is not — so the closure builds its own throwaway one. That is
        // sound only because remote mode never touches it:
        // `routed::ensure_operator` takes the hub branch before the local
        // store or ssh client is ever read.
        (
            "ensure_operator",
            "ensure_operator",
            json!({}),
            SESSION_PAYLOAD,
            Box::new(|b, _s, h| {
                let dir = tempfile::tempdir().unwrap();
                let throwaway_store = Arc::new(Mutex::new(
                    Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus))
                        .unwrap(),
                ));
                block_on(commands::operator::routed::ensure_operator(
                    b,
                    &throwaway_store,
                    h,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        // Work graph M14: the Work view's decisions.
        (
            "set_primary_work",
            "work_link",
            json!({ "session_id": 7, "action": "set_primary", "key": null, "item_id": null,
                    "link_id": 5, "source": null, "expected_primary": 4 }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::set_primary_work(
                    b,
                    commands::work_view::SetPrimaryWorkArgs {
                        session_id: 7,
                        link_id: 5,
                        expected_primary: Some(4),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "switch_session_work",
            "work_link",
            json!({ "session_id": 7, "action": "switch", "key": "PAY-2", "item_id": null,
                    "link_id": 5, "source": null, "expected_primary": 5, "ack_live": false }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::switch_session_work(
                    b,
                    commands::work_view::SwitchSessionWorkArgs {
                        session_id: 7,
                        link_id: 5,
                        key: Some("PAY-2".into()),
                        item_id: None,
                        expected_primary: Some(5),
                        ack_live: Some(false),
                        force_cross_org: false,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "reconsider_work_link",
            "work_link",
            json!({ "session_id": 7, "action": "reconsider", "key": null, "item_id": null,
                    "link_id": 5, "source": null, "expected_version": 3 }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::reconsider_work_link(
                    b,
                    commands::work_view::WorkLinkDecisionArgs {
                        session_id: 7,
                        link_id: 5,
                        expected_version: Some(3),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "ack_work_link",
            "work_link",
            json!({ "session_id": 7, "action": "ack", "key": null, "item_id": null,
                    "link_id": 5, "source": null }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::ack_work_link(
                    b,
                    commands::work_view::WorkLinkDecisionArgs {
                        session_id: 7,
                        link_id: 5,
                        expected_version: None,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "decide_work_batch",
            "work_link",
            json!({ "session_id": null, "action": "decide_batch", "key": null, "item_id": null,
                    "link_id": null, "source": null,
                    "decisions": [{ "session_id": 7, "link_id": 5, "decision": "confirm",
                                    "expected_version": 2, "primary": false }] }),
            r#"{"results":[{"link_id":5,"session_id":7,"ok":true,"version":3}]}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::decide_work_batch(
                    b,
                    commands::work_view::DecideWorkBatchArgs {
                        decisions: vec![fleet_core::service::work::structure::LinkDecision {
                            session_id: 7,
                            link_id: 5,
                            decision: "confirm".into(),
                            expected_version: Some(2),
                            primary: Some(false),
                        }],
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "place_work",
            "work_link",
            json!({ "session_id": null, "action": "place", "key": null, "item_id": null,
                    "link_id": null, "source": null, "task_id": "item:3",
                    "group": "Payments", "expected_version": 0 }),
            r#"{"task_id":"item:3","title":"t","kind":"local","unavailable":false,"mine":false,"org_source":"none","org_fenced":false,"org_mixed":false,"group":{"id":"label:Payments","label":"Payments","source":"manual","editable":true},"counts":{"active":0,"ended":0,"suggested":0},"needs_you":false,"review":false,"placement_version":1,"sessions":[],"sessions_more":0}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::place_work(
                    b,
                    commands::work_view::PlaceWorkArgs {
                        task_id: "item:3".into(),
                        group: "Payments".into(),
                        note: None,
                        expected_version: 0,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "assign_work_org",
            "work_link",
            json!({ "session_id": null, "action": "assign_org", "key": null, "item_id": null,
                    "link_id": null, "source": null, "task_id": "item:3", "org_id": 2,
                    "impact_token": "abc" }),
            r#"{"task_id":"item:3","title":"t","kind":"local","unavailable":false,"mine":false,"org_id":2,"org_source":"item","org_fenced":true,"org_mixed":false,"group":{"id":"none","label":"No group","source":"none","editable":true},"counts":{"active":0,"ended":0,"suggested":0},"needs_you":false,"review":false,"placement_version":0,"sessions":[],"sessions_more":0}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::assign_work_org(
                    b,
                    commands::work_view::AssignWorkOrgArgs {
                        task_id: "item:3".into(),
                        org_id: 2,
                        impact_token: "abc".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "save_work_rule",
            "work_link",
            json!({ "session_id": null, "action": "rule_save", "key": null, "item_id": null,
                    "link_id": null, "source": null,
                    "rule": { "name": "Pay", "conditions": { "key_prefix": "PAY" }, "group": "Payments" } }),
            r#"{"id":1,"name":"Pay","enabled":true,"version":1,"conditions":{"key_prefix":"PAY"},"group":"Payments","created_at":1,"updated_at":1}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::save_work_rule(
                    b,
                    commands::work_view::WorkRuleArgs {
                        rule: fleet_core::service::work::structure::RuleInput {
                            name: "Pay".into(),
                            conditions: fleet_core::store::RuleConditions {
                                key_prefix: Some("PAY".into()),
                                ..Default::default()
                            },
                            group: "Payments".into(),
                            ..Default::default()
                        },
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "delete_work_rule",
            "work_link",
            json!({ "session_id": null, "action": "rule_delete", "key": null, "item_id": null,
                    "link_id": null, "source": null, "rule_id": 4, "expected_version": 2 }),
            r#"{"deleted":true}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::delete_work_rule(
                    b,
                    commands::work_view::DeleteWorkRuleArgs {
                        rule_id: 4,
                        expected_version: Some(2),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "save_work_view",
            "work_link",
            json!({ "session_id": null, "action": "view_save", "key": null, "item_id": null,
                    "link_id": null, "source": null,
                    "view": { "name": "Mine", "filters": { "mine": true } } }),
            r#"{"id":1,"name":"Mine","filters":{"mine":true},"version":1,"updated_at":1}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::save_work_view(
                    b,
                    commands::work_view::SaveWorkViewArgs {
                        view: fleet_core::service::work::structure::ViewInput {
                            name: "Mine".into(),
                            filters: fleet_core::service::work::view::WorkTreeFilters {
                                mine: Some(true),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "delete_work_view",
            "work_link",
            json!({ "session_id": null, "action": "view_delete", "key": null, "item_id": null,
                    "link_id": null, "source": null, "view_id": 9, "expected_version": 1 }),
            r#"{"deleted":true}"#,
            Box::new(|b, s, _| {
                block_on(commands::work_view::routed::delete_work_view(
                    b,
                    commands::work_view::DeleteWorkViewArgs {
                        view_id: 9,
                        expected_version: Some(1),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // ── orchestration O1: missions ────────────────────────────────────
        (
            "save_mission",
            "work_link",
            json!({ "session_id": null, "action": "mission_save", "key": null, "item_id": 3,
                    "link_id": null, "source": null, "expected_version": 2, "mission_id": 4,
                    "mission": { "name": "m", "goal": "g", "level": 1 } }),
            r#"{"id":4,"name":"m","goal":"g","mode":"finite","state":"draft","level":0,"plan_version":1,"created_at":1,"updated_at":1,"version":1}"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::save_mission(
                    b,
                    commands::missions::SaveMissionArgs {
                        mission_id: Some(4),
                        item_id: Some(3),
                        expected_version: Some(2),
                        mission: fleet_core::service::work::missions::MissionInput {
                            name: Some("m".into()),
                            goal: Some("g".into()),
                            level: Some(1),
                            ..Default::default()
                        },
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "set_mission_state",
            "work_link",
            json!({ "session_id": null, "action": "mission_state", "key": null, "item_id": null,
                    "link_id": null, "source": null, "status": "active",
                    "expected_version": 1, "mission_id": 4 }),
            r#"{"id":4,"name":"m","goal":"g","mode":"finite","state":"draft","level":0,"plan_version":1,"created_at":1,"updated_at":1,"version":1}"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::set_mission_state(
                    b,
                    commands::missions::SetMissionStateArgs {
                        mission_id: 4,
                        state: "active".into(),
                        expected_version: Some(1),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "set_mission_repo",
            "work_link",
            json!({ "session_id": null, "action": "mission_repo", "key": null, "item_id": null,
                    "link_id": null, "source": null, "project_id": 8, "on": true,
                    "role": "backend", "mission_id": 4 }),
            r#"{"id":4,"name":"m","goal":"g","mode":"finite","state":"draft","level":0,"plan_version":1,"created_at":1,"updated_at":1,"version":1}"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::set_mission_repo(
                    b,
                    commands::missions::SetMissionRepoArgs {
                        mission_id: 4,
                        project_id: 8,
                        role: Some("backend".into()),
                        on: true,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "set_mission_item",
            "work_link",
            json!({ "session_id": null, "action": "mission_item", "key": null, "item_id": 3,
                    "link_id": null, "source": null, "on": false, "mission_id": 4 }),
            r#"{"id":4,"name":"m","goal":"g","mode":"finite","state":"draft","level":0,"plan_version":1,"created_at":1,"updated_at":1,"version":1}"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::set_mission_item(
                    b,
                    commands::missions::SetMissionItemArgs {
                        mission_id: 4,
                        item_id: 3,
                        on: false,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "import_mission_plan",
            "work_link",
            json!({ "session_id": null, "action": "mission_import", "key": null, "item_id": null,
                    "link_id": null, "source": null, "mission_id": 4,
                    "plan": [{ "step": "1.1", "title": "Schema", "lane": "A", "needs": ["0.9"] }] }),
            r#"{"created":1,"updated":0,"unchanged":0,"deps_added":0,"deps_removed":0}"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::import_mission_plan(
                    b,
                    commands::missions::ImportMissionPlanArgs {
                        mission_id: 4,
                        plan: vec![fleet_core::service::work::plan_import::PlanRow {
                            step: "1.1".into(),
                            title: "Schema".into(),
                            lane: Some("A".into()),
                            needs: vec!["0.9".into()],
                            status: None,
                        }],
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "delete_mission",
            "work_link",
            json!({ "session_id": null, "action": "mission_delete", "key": null, "item_id": null,
                    "link_id": null, "source": null, "mission_id": 4 }),
            r#"{"removed":4}"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::delete_mission(
                    b,
                    commands::missions::DeleteMissionArgs { mission_id: 4 },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // ── orchestration O2: the mission graph ───────────────────────────
        (
            "set_work_dep",
            "work_link",
            json!({ "session_id": null, "action": "dep", "key": null, "item_id": 3,
                    "link_id": null, "source": null, "on": true, "depends_on": 5 }),
            r#"{"item_id":3,"changed":true}"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::set_work_dep(
                    b,
                    commands::missions::SetWorkDepArgs {
                        item_id: 3,
                        depends_on: 5,
                        on: true,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "set_work_hold",
            "work_link",
            json!({ "session_id": null, "action": "hold", "key": null, "item_id": 3,
                    "link_id": null, "source": null, "on": false }),
            r#"{"item_id":3,"changed":false}"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::set_work_hold(
                    b,
                    commands::missions::SetWorkHoldArgs {
                        item_id: 3,
                        on: false,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "accept_work_proposals",
            "work_link",
            json!({ "session_id": null, "action": "accept_many", "key": null, "item_id": null,
                    "link_id": null, "source": null, "item_ids": [3, 4] }),
            r#"[]"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::accept_work_proposals(
                    b,
                    commands::missions::WorkProposalsArgs {
                        item_ids: vec![3, 4],
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "undo_work_accept",
            "work_link",
            json!({ "session_id": null, "action": "undo_accept", "key": null, "item_id": null,
                    "link_id": null, "source": null, "item_ids": [3] }),
            r#"[]"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::undo_work_accept(
                    b,
                    commands::missions::WorkProposalsArgs { item_ids: vec![3] },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "set_work_done_when",
            "work_link",
            json!({ "session_id": null, "action": "done_when", "key": null, "item_id": 3,
                    "link_id": null, "source": null, "done_when": ["review"] }),
            r#"{"item_id":3,"changed":true}"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::set_work_done_when(
                    b,
                    commands::missions::SetWorkDoneWhenArgs {
                        item_id: 3,
                        done_when: vec!["review".into()],
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "verify_work_item",
            "work_link",
            json!({ "session_id": null, "action": "verify", "key": null, "item_id": 3,
                    "link_id": null, "source": null, "line": "review", "ok": true }),
            r#"{"item_id":3,"changed":true}"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::verify_work_item(
                    b,
                    commands::missions::VerifyWorkItemArgs {
                        item_id: 3,
                        line: "review".into(),
                        ok: true,
                        note: None,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // ── the mission loop (orchestration O4–O6) ──────────────────────
        (
            "start_mission_wave",
            "work_link",
            json!({ "session_id": null, "key": null, "link_id": null, "source": null, "action": "mission_start", "item_id": null,
                    "mission_id": 1, "step": "run:3" }),
            r#"{"mission_id":1,"results":[]}"#,
            Box::new(|b, s, ssh| {
                block_on(commands::missions::routed::start_mission_wave(
                    b,
                    commands::missions::StartMissionWaveArgs {
                        mission_id: 1,
                        step: Some("run:3".into()),
                    },
                    s,
                    ssh,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "retry_work_item",
            "work_link",
            json!({ "session_id": null, "key": null, "link_id": null, "source": null, "action": "retry", "item_id": 3, "note": "smaller" }),
            r#"{"step":{"kind":"retry","item_id":3,"reason":"r","auto":false},"ok":true,"detail":"d"}"#,
            Box::new(|b, s, ssh| {
                block_on(commands::missions::routed::retry_work_item(
                    b,
                    commands::missions::RetryWorkItemArgs {
                        item_id: 3,
                        note: Some("smaller".into()),
                    },
                    s,
                    ssh,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "plan_mission",
            "work_link",
            json!({ "session_id": null, "key": null, "link_id": null, "source": null, "action": "mission_plan", "item_id": null, "mission_id": 1 }),
            r#"{"mission_id":1}"#,
            Box::new(|b, s, ssh| {
                block_on(commands::missions::routed::plan_mission(
                    b,
                    commands::missions::MissionIdArgs { mission_id: 1 },
                    s,
                    ssh,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "decide_mission_card",
            "work_link",
            json!({ "session_id": null, "key": null, "link_id": null, "source": null, "action": "card_decide", "item_id": null, "card_id": 5, "ok": false }),
            r#"{"id":5,"mission_id":1,"decision_id":"d","source":"planner","kind":"run","state":"dismissed","created_at":1}"#,
            Box::new(|b, s, ssh| {
                block_on(commands::missions::routed::decide_mission_card(
                    b,
                    commands::missions::DecideMissionCardArgs {
                        card_id: 5,
                        ok: false,
                        note: None,
                    },
                    s,
                    ssh,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "grant_mission",
            "work_link",
            json!({ "session_id": null, "key": null, "link_id": null, "source": null, "action": "mission_grant", "item_id": null, "mission_id": 1,
                    "level": 2, "hours": 4, "budget_cents": 500 }),
            r#"{"id":1,"mission_id":1,"plan_version":1,"level":2,"granted_by":"fleet","created_at":1,"expires_at":2}"#,
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::grant_mission(
                    b,
                    commands::missions::GrantMissionArgs {
                        mission_id: 1,
                        level: 2,
                        hours: Some(4),
                        budget_cents: Some(500),
                        ..Default::default()
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "revoke_mission_grant",
            "work_link",
            json!({ "session_id": null, "key": null, "link_id": null, "source": null, "action": "mission_revoke", "item_id": null, "mission_id": 1 }),
            "1",
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::revoke_mission_grant(
                    b,
                    commands::missions::MissionIdArgs { mission_id: 1 },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "pause_all_missions",
            "work_link",
            json!({ "session_id": null, "key": null, "link_id": null, "source": null, "action": "missions_pause_all", "item_id": null }),
            "[1]",
            Box::new(|b, s, _| {
                block_on(commands::missions::routed::pause_all_missions(
                    b,
                    commands::missions::PauseAllMissionsArgs {},
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "mission_release_note",
            "work_link",
            json!({ "session_id": null, "key": null, "link_id": null, "source": null, "action": "mission_release_note", "item_id": null, "mission_id": 1 }),
            r#"{"text":"t","model":"haiku","host_alias":"h","from":"1 task","at":1}"#,
            Box::new(|b, s, ssh| {
                block_on(commands::missions::routed::mission_release_note(
                    b,
                    commands::missions::MissionIdArgs { mission_id: 1 },
                    s,
                    ssh,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "today_brief",
            "work_link",
            json!({ "session_id": null, "key": null, "link_id": null, "source": null, "action": "today_brief", "item_id": null, "refresh": true, "since": 100 }),
            "{}",
            Box::new(|b, s, ssh| {
                block_on(commands::missions::routed::today_brief(
                    b,
                    commands::missions::TodayBriefArgs {
                        refresh: true,
                        since: Some(100),
                        org_id: None,
                    },
                    s,
                    ssh,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "mission_triage",
            "work_link",
            json!({ "session_id": null, "key": null, "link_id": null, "source": null, "action": "mission_triage", "item_id": null, "mission_id": 1, "refresh": true }),
            "{}",
            Box::new(|b, s, ssh| {
                block_on(commands::missions::routed::mission_triage(
                    b,
                    commands::missions::MissionTriageArgs {
                        mission_id: 1,
                        refresh: true,
                    },
                    s,
                    ssh,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        // ── multi-user M1 (T13): the three sharing mutations ─────────────
        //
        // `level` crosses as the string the user chose and is validated by
        // the store, so this case proves the field arrives at all; `narrow`
        // carries no level BECAUSE there is only one direction a grant moves
        // (spec §4.3 invariant 3), and no tool raises one.
        (
            "session_share",
            "session_share",
            json!({ "session_id": 42, "person": "jane", "level": "drive" }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::session_share(
                    b,
                    commands::sessions::SessionShareArgs {
                        session_id: 42,
                        person: "jane".into(),
                        org: None,
                        level: "drive".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "session_unshare",
            "session_unshare",
            json!({ "session_id": 42, "person": "jane" }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::session_unshare(
                    b,
                    commands::sessions::SessionGrantArgs {
                        session_id: 42,
                        person: "jane".into(),
                        org: None,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "session_narrow",
            "session_narrow",
            json!({ "session_id": 42, "person": "jane" }),
            SESSION_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::sessions::routed::session_narrow(
                    b,
                    commands::sessions::SessionGrantArgs {
                        session_id: 42,
                        person: "jane".into(),
                        org: None,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
    ]
}

/// A complete `MovePreview` (every field required — no `#[serde(default)]`,
/// per the wire rule), wrapped as a `MoveOutcome::Preview` the way a hub
/// answering a `dry_run: true` call actually does.
const PREVIEW_PAYLOAD: &str = r#"{"kind":"preview","session_id":7,"from_host":"trn","to_host":"hetzner","branch":"main","source_cwd":"/w/demo","unpushed_commits":2,"commits_ahead":null,"dirty":[{"status":" M","path":"src/lib.rs"}],"ignored_carried":[{"path":".env","bytes":4096}],"ignored_left_behind":[{"path":"node_modules/","bytes":null,"reason":"denylisted"}],"transcript_bytes":1024,"session_state_files":2,"session_state_bytes":2048,"memory_files":1,"memory_bytes":128,"target_path":"/w/demo","target":{"state":"clean","head":"1111111111111111111111111111111111111111"},"unknowns":["bundle size is decided only by snapshotting"]}"#;

/// `MoveOutcome::Preview` is otherwise untested anywhere: every routed
/// `move_session` case above answers `MOVE_PAYLOAD` (`kind: "moved"`), so
/// nothing proves the OTHER tag actually round-trips through the desktop's
/// `routed::move_session` — a `dry_run: true` call sent to the hub and a
/// `MovePreview` sent back.
#[test]
fn a_hub_answering_a_preview_deserialises_into_move_outcome_preview() {
    use fleet_core::service::move_session::{MoveOutcome, MoveSessionArgs};

    let fake = Fake::answering(PREVIEW_PAYLOAD);
    let (_dir, st) = store();
    let got = block_on(commands::move_session::routed::move_session(
        &remote_backend(&fake),
        MoveSessionArgs {
            session_id: 7,
            target_host_alias: "hetzner".into(),
            keep_source: false,
            strict: false,
            clean_target: false,
            dry_run: true,
            when: fleet_core::service::move_session::When::Now,
            force_cross_org: false,
        },
        &st,
        &ssh(),
    ))
    .expect("a preview answer must deserialise as the command's return type");
    match got {
        MoveOutcome::Preview(p) => assert_eq!(p.session_id, 7),
        other => panic!("expected MoveOutcome::Preview, got {other:?}"),
    }
    let (tool, args) = fake.only_call();
    assert_eq!(tool, "move_session");
    assert_eq!(args["dry_run"], true);
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

/// `health_check` used to be exempt because it returned a bare `Health`; in
/// remote mode it therefore read the local database, which a hub client
/// never fills, and answered a **zeroed fleet**
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
    // `health_check` is the one routed command the two case tables do not
    // drive, so this is where its VERDICTS row gets the cross-check `check`
    // does for the other 35: the row's tool against the tool the request
    // actually carried, not against a second hand-typed literal.
    // ROUTED_WITHOUT_A_CASE names this test for exactly this line.
    assert_eq!(
        verdicts::verdict("health_check").and_then(Verdict::tool),
        Some(tool.as_str()),
        "health_check sent {tool}, but its VERDICTS row names {:?} — that row is \
         published to the frontend and the docs",
        verdicts::verdict("health_check").and_then(Verdict::tool),
    );
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

/// A hub that never answers, and a flag for whether the call reached it.
struct Hanging(std::sync::atomic::AtomicBool);

#[async_trait::async_trait]
impl remote::HubTransport for Hanging {
    async fn post_json(
        &self,
        _url: &str,
        _bearer: &str,
        _body: String,
    ) -> Result<remote::HubResponse, String> {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        std::future::pending().await
    }
}

/// #352: Stop waiting on a hub client. The dialog cancels by `call_id`; the
/// hub branch used to bind nothing under it, so the cancel was a no-op and
/// the dialog stayed busy for the hub's whole deadline. Now the call comes
/// back `E_CANCELLED`, says the hub may still finish (D3: no hub-side
/// cancel), and carries the GitHub hedge for a `create_remote` run.
#[test]
fn stop_waiting_on_a_hub_client_abandons_add_project() {
    use fleet_core::cancel::CancellationRegistry;
    use fleet_core::service::add_project::{AddProjectArgs, AddProjectSource};
    for (source, github) in [
        (
            AddProjectSource::Clone {
                url: "o/r".into(),
                existing: false,
            },
            false,
        ),
        (
            AddProjectSource::New {
                owner: "o".into(),
                repo: "r".into(),
                create_remote: true,
                confirm: Some("tok".into()),
            },
            true,
        ),
    ] {
        let hub = Arc::new(Hanging(Default::default()));
        let link = Arc::new(connection::HubConnectionStatus::remote(
            Arc::new(Silent),
            &cfg().token,
        ));
        link.report(connection::HubConnection::Connected);
        let backend = FleetBackend::remote_over(cfg(), hub.clone())
            .watching(link as Arc<dyn connection::ConnectionView>);
        let (_dir, st) = store();
        let reg = CancellationRegistry::new();
        let ssh = ssh();
        let e = block_on(async {
            let call = commands::projects::routed::add_project(
                &backend,
                AddProjectArgs {
                    host_alias: "trn".into(),
                    source,
                    call_id: Some(41),
                },
                &st,
                &ssh,
                &reg,
            );
            let stop = async {
                while !hub.0.load(std::sync::atomic::Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
                reg.cancel(41);
                std::future::pending::<()>().await
            };
            let r = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                tokio::select! {
                    r = call => r,
                    () = stop => unreachable!(),
                }
            })
            .await
            .expect("cancel_command(call_id) must end the wait for the hub");
            match r {
                Err(e) => e,
                Ok(_) => panic!("a stopped wait must not look like a created project"),
            }
        });
        assert_eq!(e.code, codes::E_CANCELLED, "{e:?}");
        assert!(
            e.message.contains("the hub may still finish"),
            "{}",
            e.message
        );
        assert!(e.message.contains("trn"), "{}", e.message);
        assert_eq!(e.message.contains("GitHub"), github, "{}", e.message);
    }
}

// ── 2. standalone mode still runs the local service call ────────────────────

/// The store-backed reads answer from the store. This is the "standalone
/// behaviour must not change" constraint, asserted per command.
#[test]
fn standalone_reads_still_come_from_the_local_store() {
    // `app_version::get()` panics until the binary declares its version, and a
    // test binary never runs `run()`. The health assertion below reads it.
    crate::declare_app_version();
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

/// Multi-user M1 (T13): the standalone arm of the sharing commands.
///
/// The thing under test is the one decision this layer makes that the hub
/// makes differently — **who the caller is**. On a hub the person comes off
/// the connection; standalone there is no connection, so the `None` arm has
/// to resolve the fleet's own personal owner (migration 100). Getting that
/// wrong is not a visible error: `person_id: null` is exactly what
/// `src/lib/access.ts` reads as "we are nobody", and a client that is nobody
/// holds no grants, so every session shared with this person would quietly
/// stop being reachable while nothing failed.
///
/// A session row cannot be seeded from this crate (`Store::upsert_session` is
/// `#[cfg(test)]` inside fleet-core), so the grant list is empty here by
/// construction; `store/session_grants.rs` owns the filled case. What this
/// proves is the identity, and that the local path ran at all.
#[test]
fn the_standalone_sharing_arm_is_this_fleets_own_person() {
    let (_dir, st) = store();
    let local = FleetBackend::local();

    let owner = st
        .lock()
        .unwrap()
        .personal_owner_id()
        .unwrap()
        .expect("a fresh store has a personal owner (migration 100)");

    let mine = block_on(commands::sessions::routed::my_grants(&local, &st)).expect("my_grants");
    assert_eq!(
        mine.person_id,
        Some(owner),
        "the standalone arm must answer the fleet's own person; None here reads as \
         `we are nobody` on the client and silently drops every grant"
    );
    assert!(mine.grants.is_empty(), "a fresh store has no grants");

    // The local path ran, rather than a hub answering: `person_named` is the
    // local store's own refusal for a name nobody holds.
    let err = block_on(commands::sessions::routed::session_share(
        &local,
        commands::sessions::SessionShareArgs {
            session_id: 1,
            person: "nobody-by-that-name".into(),
            org: None,
            level: "watch".into(),
        },
        &st,
    ))
    .expect_err("sharing with a person this fleet does not know must refuse");
    assert_eq!(
        err.code,
        fleet_core::ipc_error::codes::E_NOTFOUND,
        "{err:?}"
    );

    // And the owner's own grant list for a session that does not exist is
    // empty, not an error and not someone else's.
    let grants = block_on(commands::sessions::routed::session_access(
        &local,
        commands::sessions::SessionAccessArgs { session_id: 1 },
        &st,
    ))
    .expect("session_access");
    assert!(grants.is_empty());
}

/// Multi-user M1 (T13): **every sharing command addresses its session by ROW
/// ID only.**
///
/// The `host_alias` + `tmux_name` pair every other session command accepts is
/// reusable — the next session started on a host can take a dead one's tmux
/// name — so a grant resolved by name could land on a different row than the
/// one the owner was looking at. The tools refuse the pair for exactly this
/// reason (`mcp/tools/sharing.rs`'s header); this holds the desktop's
/// argument structs to the same shape, because an added field here would be
/// serialised straight through `route` to a tool that would then have to
/// start ignoring it.
#[test]
fn the_sharing_commands_address_a_session_by_id_and_nothing_reusable() {
    use commands::sessions::{
        CaptureSessionArgs, SessionAccessArgs, SessionGrantArgs, SessionShareArgs,
    };

    fn keys<T: serde::Serialize>(v: &T) -> BTreeSet<String> {
        match serde_json::to_value(v).expect("the args must serialise") {
            Value::Object(m) => m.keys().cloned().collect(),
            other => panic!("expected an object on the wire, got {other}"),
        }
    }
    fn want(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    assert_eq!(
        keys(&SessionShareArgs {
            session_id: 1,
            person: "jane".into(),
            org: None,
            level: "watch".into(),
        }),
        want(&["session_id", "person", "level"])
    );
    assert_eq!(
        keys(&SessionGrantArgs {
            session_id: 1,
            person: "jane".into(),
            org: None,
        }),
        want(&["session_id", "person"])
    );
    assert_eq!(
        keys(&SessionAccessArgs { session_id: 1 }),
        want(&["session_id"])
    );
    assert_eq!(
        keys(&CaptureSessionArgs {
            session_id: 1,
            scrollback_lines: None,
            max_lines: None,
        }),
        want(&["session_id", "scrollback_lines", "max_lines"])
    );

    // A client that sends the reusable pair anyway gets it DROPPED, not
    // forwarded: the structs have no field for it, so it never reaches a tool.
    let a: SessionGrantArgs = serde_json::from_value(
        json!({ "session_id": 1, "person": "jane", "host_alias": "trn", "tmux_name": "demo" }),
    )
    .expect("unknown keys are ignored, as every args struct in this file does");
    assert_eq!(keys(&a), want(&["session_id", "person"]));
}

/// Redesign 11.2: removing a member standalone says what happened to their
/// shares in the local store, for each of the dialog's three choices.
#[test]
fn standalone_remove_member_takes_each_grants_choice() {
    let (_dir, st) = store();
    let local = FleetBackend::local();
    let (org, people) = {
        let s = st.lock().unwrap();
        let org = s.add_org("Acme", None, false).unwrap().id;
        let people: Vec<i64> = ["ann", "ben", "cy"]
            .iter()
            .map(|n| {
                let p = s.create_person(n, None).unwrap().id;
                s.set_org_member(org, p, "member", None).unwrap();
                p
            })
            .collect();
        (org, people)
    };
    let remove = |person_id: i64, grants: &str| {
        block_on(commands::orgs::routed::remove_org_member_choosing(
            &local,
            &st,
            commands::orgs::RemoveOrgMemberArgs {
                org_id: org,
                person_id,
                grants: Some(grants.into()),
                ..Default::default()
            },
        ))
    };
    assert_eq!(
        remove(people[0], "narrow").unwrap(),
        json!({ "removed": true, "revoked_grants": 0, "narrowed": 0 })
    );
    assert_eq!(remove(people[1], "keep").unwrap()["removed"], true);
    assert_eq!(remove(people[2], "revoke").unwrap()["removed"], true);
    assert_eq!(remove(people[2], "all").unwrap_err().code, codes::E_INVALID);
    assert!(st.lock().unwrap().org_members(org).unwrap().is_empty());
}

/// The work-link commands answer from the local store when standalone.
/// Proof without seeding a session (no public way from this crate): the read
/// comes back empty and every decision answers the local store's
/// `E_NOTFOUND` for an unknown session — the hub arm would have returned a
/// payload instead.
#[test]
fn standalone_work_links_are_decided_in_the_local_store() {
    let (_dir, st) = store();
    let local = FleetBackend::local();
    let links = block_on(commands::work::routed::session_work_links(
        &local,
        fleet_core::service::work::WorkArgs {
            session_id: Some(99),
            ..Default::default()
        },
        &st,
    ))
    .expect("links");
    assert!(links.is_empty());
    let errs = [
        block_on(commands::work::routed::link_session_work(
            &local,
            commands::work::LinkSessionWorkArgs {
                session_id: 99,
                key: Some("ABC-1".into()),
                item_id: None,
                force_cross_org: false,
                ..Default::default()
            },
            &st,
        )),
        block_on(commands::work::routed::reject_session_work(
            &local,
            commands::work::RejectSessionWorkArgs {
                session_id: 99,
                key: Some("ABC-1".into()),
                item_id: None,
                link_id: None,
                ..Default::default()
            },
            &st,
        ))
        .map(|d| d.row),
        block_on(commands::work::routed::unlink_session_work(
            &local,
            commands::work::UnlinkSessionWorkArgs {
                session_id: 99,
                link_id: 1,
                ..Default::default()
            },
            &st,
        )),
    ];
    for r in errs {
        assert_eq!(r.expect_err("unknown session").code, codes::E_NOTFOUND);
    }
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

/// `list_host_worktrees` used to refuse in remote mode and now routes, so
/// its standalone arm is the half that could have been lost in the move:
/// the scan of `local` answers from this app's own store, without a hub and
/// without SSH.
#[test]
fn standalone_list_host_worktrees_still_scans_from_the_local_store() {
    use fleet_core::service::worktrees::ListHostWorktreesArgs;
    let (_dir, st) = store();
    let pid = {
        let s = st.lock().unwrap();
        let pid = s.upsert_project("o", "r", "/p").unwrap();
        s.upsert_worktree(pid, "main", "/p", Some("main")).unwrap();
        pid
    };
    let out = block_on(commands::worktrees::routed::list_host_worktrees(
        &FleetBackend::local(),
        ListHostWorktreesArgs {
            host_alias: "local".into(),
            project_id: pid,
        },
        &st,
        &ssh(),
    ))
    .expect("the local store answers");
    assert_eq!(out.host_alias, "local");
    assert!(out.cloned);
    assert_eq!(out.worktrees.len(), 1);
    assert_eq!(out.worktrees[0].name, "main");
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
    for (_, tool, _, _, run) in routed_read_cases()
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
        .refuse_local_only("provision_hosts")
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

// ── 2c. a hub whose wire contract this build does not read ──────────────────

/// A sink that throws the connection event away: this is about what the
/// status REMEMBERS, which is what the gate in `remote.rs` reads.
struct Silent;

impl crate::backend::events::RemoteEventSink for Silent {
    fn emit_remote(&self, _name: &'static str, _payload: Value) {}
}

/// A hub client whose last `ready` frame named a wire-contract revision
/// outside this build's range. The status is the production one, reported
/// into the way the event bridge reports into it.
fn skewed_backend(fake: &Arc<Fake>, state: connection::HubConnection) -> FleetBackend {
    let link = Arc::new(connection::HubConnectionStatus::remote(
        Arc::new(Silent),
        &cfg().token,
    ));
    link.report(state);
    FleetBackend::remote_over(cfg(), fake.clone())
        .watching(link as Arc<dyn connection::ConnectionView>)
}

/// The command half of #148's check. The event bridge already applies no row
/// event and runs no backfill from a skewed hub — but a routed read or
/// mutation deserialises that hub's answer into the very same row types, with
/// `#[serde(default)]` on every optional field, so a renamed column becomes a
/// silent default in the stores. Every routed command must refuse instead,
/// and must not reach the network: the point is that nothing comes back to
/// deserialise.
#[test]
fn a_hub_with_a_skewed_wire_contract_refuses_every_routed_command() {
    for state in [
        connection::HubConnection::HubTooOld {
            hub_contract: 1,
            min_contract: 3,
        },
        connection::HubConnection::HubTooNew {
            hub_contract: 9,
            max_contract: 1,
        },
    ] {
        for (_, tool, _, payload, run) in routed_read_cases()
            .into_iter()
            .chain(routed_mutation_cases())
        {
            let fake = Fake::answering(payload);
            let (_dir, st) = store();
            let err = run(&skewed_backend(&fake, state.clone()), &st, &ssh()).expect_err(&format!(
                "{tool} read a hub whose row shapes this build cannot trust"
            ));
            assert_eq!(err.code, codes::E_HUB_CONTRACT, "{tool}: {err:?}");
            assert!(
                err.message.contains("wire contract is revision"),
                "{tool}: the refusal must name the revisions: {}",
                err.message
            );
            fake.was_not_called();
        }
        // `health_check` is the one routed command the case tables do not
        // drive (`ROUTED_WITHOUT_A_CASE`), so the sweep above cannot reach
        // it. It goes through the same `route` → `call` → `call_text`, and
        // the footer reading a skewed hub's fleet is as wrong as the sidebar
        // doing it, so it is driven here by hand.
        let fake = Fake::answering("{}");
        let (_dir, st) = store();
        let err = block_on(commands::health::routed::health_check(
            &skewed_backend(&fake, state.clone()),
            &st,
        ))
        // `Health` is not `Debug`, so `expect_err` is not available.
        .err()
        .expect("the footer must not show a skewed hub's fleet either");
        assert_eq!(err.code, codes::E_HUB_CONTRACT, "health_check: {err:?}");
        fake.was_not_called();
    }
}

// ── 2d. a dry run needs a hub this launch has CONFIRMED ─────────────────────

fn dry_run_args(dry_run: bool) -> fleet_core::service::move_session::MoveSessionArgs {
    fleet_core::service::move_session::MoveSessionArgs {
        session_id: 7,
        target_host_alias: "hetzner".into(),
        keep_source: false,
        strict: false,
        clean_target: false,
        dry_run,
        when: fleet_core::service::move_session::When::Now,
        force_cross_org: false,
    }
}

/// A hub client whose link has not yet judged any `ready` frame.
fn unconfirmed_backend(fake: &Arc<Fake>) -> FleetBackend {
    let link = Arc::new(connection::HubConnectionStatus::remote(
        Arc::new(Silent),
        &cfg().token,
    ));
    FleetBackend::remote_over(cfg(), fake.clone())
        .watching(link as Arc<dyn connection::ConnectionView>)
}

/// A hub built before `dry_run` existed ignores the flag and performs a REAL
/// move. The contract gate only refuses such a hub once its `ready` frame has
/// been judged; before that, "no verdict" means "never judged" as much as
/// "in range". So a dry run is refused until this launch has positively seen
/// an in-range `ready` frame — before the first frame, and on a backend with
/// no link to consult at all — and the hub is never called.
#[test]
fn a_dry_run_is_refused_until_this_launch_has_confirmed_the_hub() {
    for (what, backend_of) in [
        (
            "before any ready frame",
            unconfirmed_backend as fn(&Arc<Fake>) -> FleetBackend,
        ),
        ("with no link to consult", |fake: &Arc<Fake>| {
            FleetBackend::remote_over(cfg(), fake.clone())
        }),
    ] {
        let fake = Fake::answering(PREVIEW_PAYLOAD);
        let (_dir, st) = store();
        let err = block_on(commands::move_session::routed::move_session(
            &backend_of(&fake),
            dry_run_args(true),
            &st,
            &ssh(),
        ))
        .expect_err(what);
        assert_eq!(err.code, codes::E_HUB_CONTRACT, "{what}: {err:?}");
        assert!(
            err.message.contains("confirmed the hub's version"),
            "{what}: {}",
            err.message
        );
        fake.was_not_called();
    }
}

#[test]
fn a_dry_run_routes_once_an_in_range_ready_frame_has_been_seen() {
    let fake = Fake::answering(PREVIEW_PAYLOAD);
    let (_dir, st) = store();
    let link = Arc::new(connection::HubConnectionStatus::remote(
        Arc::new(Silent),
        &cfg().token,
    ));
    let backend = FleetBackend::remote_over(cfg(), fake.clone())
        .watching(Arc::clone(&link) as Arc<dyn connection::ConnectionView>);
    link.report(connection::HubConnection::Connected);
    block_on(commands::move_session::routed::move_session(
        &backend,
        dry_run_args(true),
        &st,
        &ssh(),
    ))
    .expect("a confirmed hub takes a dry run");
    let (tool, args) = fake.only_call();
    assert_eq!(tool, "move_session");
    assert_eq!(args["dry_run"], true);
}

/// I4: the event stream dropping withdraws the confirmation — the hub that
/// answers the reconnect may be an older build — so a dry run is refused
/// again until a new `ready` frame is judged in range.
#[test]
fn a_dry_run_is_refused_again_after_the_stream_drops() {
    let fake = Fake::answering(PREVIEW_PAYLOAD);
    let (_dir, st) = store();
    let link = Arc::new(connection::HubConnectionStatus::remote(
        Arc::new(Silent),
        &cfg().token,
    ));
    let backend = FleetBackend::remote_over(cfg(), fake.clone())
        .watching(Arc::clone(&link) as Arc<dyn connection::ConnectionView>);
    link.report(connection::HubConnection::Connected);
    link.report(connection::HubConnection::Reconnecting {
        attempt: 1,
        retry_in_secs: 1,
        reason: "stream ended".into(),
    });
    let err = block_on(commands::move_session::routed::move_session(
        &backend,
        dry_run_args(true),
        &st,
        &ssh(),
    ))
    .expect_err("an unconfirmed reconnect");
    assert_eq!(err.code, codes::E_HUB_CONTRACT, "{err:?}");
    fake.was_not_called();
}

/// An out-of-range `ready` frame is already refused by the contract gate,
/// and that refusal — which names both revisions and says what to update —
/// is the one a dry run gets, not the vaguer "not yet confirmed".
#[test]
fn a_dry_run_against_a_skewed_hub_gets_the_contract_refusal() {
    let fake = Fake::answering(PREVIEW_PAYLOAD);
    let (_dir, st) = store();
    let err = block_on(commands::move_session::routed::move_session(
        &skewed_backend(
            &fake,
            connection::HubConnection::HubTooOld {
                hub_contract: 1,
                min_contract: 2,
            },
        ),
        dry_run_args(true),
        &st,
        &ssh(),
    ))
    .expect_err("a skewed hub");
    assert_eq!(err.code, codes::E_HUB_CONTRACT);
    assert!(
        err.message.contains("wire contract is revision 1"),
        "{}",
        err.message
    );
    fake.was_not_called();
}

/// The confirmation gates previews only: a real move before any `ready`
/// frame routes exactly as it always has.
#[test]
fn a_real_move_is_not_gated_on_the_confirmation() {
    let fake = Fake::answering(MOVE_PAYLOAD);
    let (_dir, st) = store();
    block_on(commands::move_session::routed::move_session(
        &unconfirmed_backend(&fake),
        dry_run_args(false),
        &st,
        &ssh(),
    ))
    .expect("a real move routes");
    let (tool, args) = fake.only_call();
    assert_eq!(tool, "move_session");
    assert_eq!(args["dry_run"], false);
}

// ── 2e. `when` widens the same guard a dry run uses ─────────────────────────

fn when_args(
    when: fleet_core::service::move_session::When,
) -> fleet_core::service::move_session::MoveSessionArgs {
    fleet_core::service::move_session::MoveSessionArgs {
        session_id: 7,
        target_host_alias: "hetzner".into(),
        keep_source: false,
        strict: false,
        clean_target: false,
        dry_run: false,
        when,
        force_cross_org: false,
    }
}

/// A hub built before `when` existed ignores it and performs a REAL move —
/// harmless for `idle` (a busy source is refused as today) but not for
/// `cancel`: cancelling a wait would instead MOVE the session (spec §3,
/// the hazard Task 4 exists to close). So `when != now` gets exactly the
/// same "not yet confirmed" guard `dry_run` already has; `now` stays
/// ungated (covered by `a_real_move_is_not_gated_on_the_confirmation`
/// above).
#[test]
fn a_when_other_than_now_is_refused_until_this_launch_has_confirmed_the_hub() {
    use fleet_core::service::move_session::When;
    for when in [When::Idle, When::Cancel] {
        let fake = Fake::answering(MOVE_PAYLOAD);
        let (_dir, st) = store();
        let err = block_on(commands::move_session::routed::move_session(
            &unconfirmed_backend(&fake),
            when_args(when),
            &st,
            &ssh(),
        ))
        .unwrap_err();
        assert_eq!(err.code, codes::E_HUB_CONTRACT, "{when:?}: {err:?}");
        assert!(
            err.message.contains("confirmed the hub's version"),
            "{when:?}: {}",
            err.message
        );
        // M3: a wait or a cancel is not a preview, and the refusal must not
        // call it one; it names the actual hazard instead.
        assert!(
            !err.message.to_lowercase().contains("preview"),
            "{when:?}: {}",
            err.message
        );
        assert!(
            err.message.contains("move the session"),
            "{when:?}: {}",
            err.message
        );
        fake.was_not_called();
    }
}

/// Once this launch has seen an in-range `ready` frame, every `when` value
/// routes — the guard exists only for the unconfirmed window.
#[test]
fn every_when_value_routes_once_an_in_range_ready_frame_has_been_seen() {
    use fleet_core::service::move_session::When;
    for when in [When::Now, When::Idle, When::Cancel] {
        let fake = Fake::answering(MOVE_PAYLOAD);
        let (_dir, st) = store();
        let link = Arc::new(connection::HubConnectionStatus::remote(
            Arc::new(Silent),
            &cfg().token,
        ));
        let backend = FleetBackend::remote_over(cfg(), fake.clone())
            .watching(Arc::clone(&link) as Arc<dyn connection::ConnectionView>);
        link.report(connection::HubConnection::Connected);
        block_on(commands::move_session::routed::move_session(
            &backend,
            when_args(when),
            &st,
            &ssh(),
        ))
        .unwrap_or_else(|e| panic!("{when:?}: {e:?}"));
        let (tool, args) = fake.only_call();
        assert_eq!(tool, "move_session");
        assert_eq!(
            args["when"],
            serde_json::to_value(when).unwrap(),
            "{when:?}"
        );
    }
}

const WAITING_PAYLOAD: &str =
    r#"{"kind":"waiting","session_id":7,"to_host":"hetzner","deadline_unix":1234567890}"#;

/// `MoveOutcome::Waiting` is otherwise untested anywhere: nothing else
/// proves a hub's `when: idle` answer (deferring the move) round-trips
/// through the desktop's `routed::move_session` into the right variant.
#[test]
fn a_hub_answering_a_wait_deserialises_into_move_outcome_waiting() {
    use fleet_core::service::move_session::{MoveOutcome, When};

    let fake = Fake::answering(WAITING_PAYLOAD);
    let (_dir, st) = store();
    let got = block_on(commands::move_session::routed::move_session(
        &remote_backend(&fake),
        when_args(When::Idle),
        &st,
        &ssh(),
    ))
    .expect("a waiting answer must deserialise as the command's return type");
    match got {
        MoveOutcome::Waiting(w) => assert_eq!(w.session_id, 7),
        other => panic!("expected MoveOutcome::Waiting, got {other:?}"),
    }
    let (tool, args) = fake.only_call();
    assert_eq!(tool, "move_session");
    assert_eq!(args["when"], "idle");
}

// ── 3. the local-only refusals ──────────────────────────────────────────────

#[test]
fn a_local_only_command_is_a_no_op_when_standalone() {
    assert!(FleetBackend::local()
        .refuse_local_only("catalog_spawn_author_session")
        .is_ok());
}

#[test]
fn a_local_only_command_refuses_in_remote_mode_and_says_where_to_go() {
    let fake = Fake::answering("[]");
    let err = remote_backend(&fake)
        .refuse_local_only("provision_hosts")
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

/// The fail-closed arm of [`FleetBackend::refuse_local_only`]: a command the
/// table cannot answer is refused anyway, never allowed.
///
/// It cannot be *called* with such a name from here — the `debug_assert!`
/// fires first in a test build, which is the point of it. What is checkable is
/// everything around that assert: that the lookup really does miss (both ways
/// it can miss), that the fallback sentence still makes a whole refusing
/// message, and that a miss does not turn a standalone app's command into an
/// error. `every_refusal_names_a_command_the_table_can_refuse` is what makes
/// the miss unshippable in the first place.
#[test]
fn a_command_the_table_cannot_answer_is_still_refused() {
    assert!(verdicts::verdict("no_such_command").is_none());
    assert!(
        verdicts::verdict("probe_host").unwrap().instead().is_none(),
        "a routed command has no sentence either, and that is the other miss"
    );

    let fake = Fake::answering("[]");
    let err = remote_backend(&fake)
        .local_only("no_such_command", NO_SENTENCE)
        .expect_err("a miss must never be a silent allow");
    assert_eq!(err.code, codes::E_LOCAL_ONLY);
    assert!(err.message.contains("no_such_command"), "{}", err.message);
    assert!(
        err.message.contains("a bug in the app"),
        "and it must say whose fault it is: {}",
        err.message
    );
    fake.was_not_called();

    assert!(
        FleetBackend::local()
            .local_only("no_such_command", NO_SENTENCE)
            .is_ok(),
        "standalone owns its own fleet; a missing row is not its problem"
    );
}

/// #146: `repair_session` routes only
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
        .refuse_local_only("catalog_spawn_author_session")
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
/// The asset catalog used to be the bulk of this list: its commands refused
/// while the hub had a tool for most of them. They all route to
/// `catalog_admin` now, including `catalog_import_host` (Task 6: import
/// works from any host over SSH); `catalog_spawn_author_session` is the one
/// left refusing, honestly — the hub genuinely has no tool that starts a
/// Claude session in its checkout.
///
/// The sentences used to be read back out of the source, because a
/// `#[tauri::command]` cannot be called without a live `tauri::App`. They are
/// in [`VERDICTS`] now, so this asks the table.
#[test]
fn a_refusal_that_has_a_hub_tool_names_it_rather_than_denying_it() {
    fn reason(command: &str) -> &'static str {
        verdicts::verdict(command)
            .unwrap_or_else(|| panic!("no verdict for {command}"))
            .instead()
            .unwrap_or_else(|| panic!("{command} no longer refuses"))
    }
    const DENIALS: [&str; 2] = ["exposes no authoring tool", "exposes no tool"];

    let said = reason("dismiss_agent_session");
    for d in DENIALS {
        assert!(!said.contains(d), "dismiss_agent_session: {said}");
    }
    assert!(
        said.contains("kill_session"),
        "the routed Kill does this for an inactive agent, and the user should \
         be sent there: {said}"
    );
}

/// True if `s` contains a plan-step reference of the form "Task" followed by
/// a number — the kind of thing that means something to whoever wrote the
/// implementation plan and nothing to a user reading an error message.
/// Hand-rolled rather than a `regex` dependency: `src-tauri` does not
/// otherwise need one.
fn contains_plan_step_reference(s: &str) -> bool {
    let mut rest = s;
    while let Some(idx) = rest.find("Task ") {
        rest = &rest[idx + "Task ".len()..];
        if rest.starts_with(|c: char| c.is_ascii_digit()) {
            return true;
        }
    }
    false
}

/// A refusal sentence is read by a user who never saw the plan that
/// introduced the command. It must not lean on plan-step vocabulary to make
/// its point.
#[test]
fn no_refusal_sentence_names_a_plan_step() {
    let offenders: Vec<&str> = VERDICTS
        .iter()
        .filter_map(|(command, verdict)| {
            let sentence = verdict.instead()?;
            contains_plan_step_reference(sentence).then_some(*command)
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "these commands' refusal sentences name a plan step a reader cannot \
         resolve: {offenders:?}"
    );
}

// ── 3b. the refusal messages, byte for byte ─────────────────────────────────

/// The recorded messages, relative to `src-tauri/` (`CARGO_MANIFEST_DIR`).
const LOCAL_ONLY_GOLDEN_PATH: &str = "src/backend/local_only.golden.json";
/// Set to rewrite the fixture. Regenerating it is never part of a refactor:
/// the whole point of the file is that a message which changed shows up as a
/// diff someone has to read.
const REGEN_LOCAL_ONLY: &str = "REGEN_LOCAL_ONLY";

/// `command -> the whole E_LOCAL_ONLY message`, rendered from the table
/// through the refusal a command actually calls, for the fixed hub of [`cfg`].
///
/// The fixture this feeds was generated the same way from the *pasted*
/// sentences, before they moved into [`VERDICTS`] (commit "pin every
/// E_LOCAL_ONLY message in a fixture"). So a green run here is the statement
/// that the table says, word for word, what the ~74 call sites used to say.
fn local_only_messages() -> String {
    let fake = Fake::answering("[]");
    let backend = remote_backend(&fake);
    let rendered: BTreeMap<&str, String> = VERDICTS
        .iter()
        .filter(|(_, v)| v.instead().is_some())
        .map(|(command, _)| {
            let err = backend
                .refuse_local_only(command)
                .expect_err("a local-only command must refuse in remote mode");
            assert_eq!(err.code, codes::E_LOCAL_ONLY, "{command}: {err:?}");
            (*command, err.message)
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
    let actual = local_only_messages();
    // 50, not the 70 it once was: the asset catalog's ~27 refusals became
    // routes to `catalog_admin` (a granted client manages the hub's catalog).
    assert!(
        actual.lines().count() > 50,
        "only {} lines of refusal — the table lost rows, or this stopped \
         rendering them",
        actual.lines().count()
    );

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
/// `cancel_command`                    -> ("commands/cancel.rs", "cancel_command")
///
/// A bare entry is registered under a `use` at the top of `lib.rs`, so the
/// file it lives in is that import's, not `lib.rs`. This used to answer
/// `lib.rs` and nobody noticed, because the only bare entry was on the
/// exception list and the lookup never ran.
fn registered_commands() -> Vec<(String, String)> {
    /// `use commands::cancel::cancel_command;` -> "commands/cancel.rs".
    fn imported_from(lib: &str, name: &str) -> String {
        let want = format!("::{name};");
        for line in lib.lines() {
            let line = line.trim();
            if let Some(path) = line.strip_prefix("use ") {
                if path.ends_with(&want) {
                    let parts: Vec<&str> = path.trim_end_matches(';').split("::").collect();
                    if let ["commands", module, _] = parts.as_slice() {
                        return format!("commands/{module}.rs");
                    }
                }
            }
        }
        "lib.rs".to_string()
    }

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
            [_] => imported_from(lib, &name),
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

/// **The desktop half of multi-user M1's review of what `main` added** — the
/// nine commands that reached this table while M1 was being built, with the
/// person-fence judgement each one was missing, and what it acts on.
///
/// Why it is needed at all. `every_command_has_a_verdict` holds this crate to
/// a ROUTING decision per command; it has nothing to say about privacy. The
/// routing verdict is nonetheless where the privacy answer lives for a hub
/// client, because `Routed` means the hub's own tool runs the call — and the
/// hub is where M1's fences are. So the row for each of these is a pair of
/// claims a reader can check: the verdict is `Routed` to the named tool, and
/// the fence is that tool's (recorded in `fleet_core`'s
/// `SESSION_REACH` / `WORK_ACTION_REACH` / `WORK_ACTION_NO_GATE` /
/// `REVIEWED_WITHOUT_A_SESSION`).
///
/// **A standalone desktop adds no fence of its own, and that is deliberate.**
/// Its store has one person — the owner `personal_owner_id` mints — so there
/// is nobody for a fence to keep out; spec §4.3's D1 promises exactly that
/// nothing changes for a single user. The moment this desktop is a window onto
/// a hub, every one of these calls is the hub's to judge.
const M1_REVIEWED_DESKTOP_COMMANDS: &[(&str, &str, &str)] = &[
    (
        "create_work_task",
        "work_link",
        "a NEW work item (a task or subtask). No session is named; `work::local::create_task` refuses every scoped caller outright",
    ),
    (
        "accept_work_proposal",
        "work_link",
        "a person's decision on an agent's proposal, by `item_id` — `work::local::decide`, which refuses every scoped caller because an agent never accepts its own proposal",
    ),
    (
        "reject_work_proposal",
        "work_link",
        "the same decision, the other way. In its session-addressed shape `reject` is the LINK decision instead and takes the hub tail's `Reach::Drive`",
    ),
    (
        "list_guides",
        "guide",
        "the fleet's page catalog, read: the live guides and the proposals \
         waiting. A `guide_proposals` row records the proposing caller's \
         LABEL (`host:<alias>`, `client:<name>`, `master` — \
         `Caller::label`) and never a session, a pane or a tmux name. There \
         is no desktop command for the agent's own `propose`: that arm is \
         reached over the control API by the host session carrying the \
         fleet-guides skill, and its row is `fleet_core`'s \
         `REVIEWED_WITHOUT_A_SESSION`",
    ),
    (
        "decide_guide",
        "guide",
        "approving or rejecting one guide — a person's, through the hub's `guide_decider`",
    ),
    ("remove_guide", "guide", "retiring one live guide"),
    (
        "list_forms",
        "ask",
        "the hub filters forms to sessions this device may read",
    ),
    (
        "get_form",
        "ask",
        "the hub gates the form's session with Reach::Read",
    ),
    (
        "answer_form",
        "ask",
        "the hub gates the form's session with Reach::Drive and refuses host tokens",
    ),
    (
        "decline_form",
        "ask",
        "the hub gates the form's session with Reach::Drive and refuses host tokens",
    ),
    (
        "catalog_set_host_harnesses",
        "catalog_admin",
        "which harnesses a HOST serves, i.e. what the next `apply_sync` \
         writes to that host's filesystem. It is one of the dozen \
         `catalog_*` commands that route to the hub's `catalog_admin`, which \
         the operator's per-client `assets` grant fences and a person never \
         does: the catalog is layers, checkouts, secrets and syncs, and the \
         one place `service/catalog/` touches a session row is \
         `catalog_spawn_author_session`, which CREATES one and stamps \
         `hub_personal_owner` on it",
    ),
];

/// Every row of [`M1_REVIEWED_DESKTOP_COMMANDS`] still describes the command
/// it names: the table has a verdict for it, and that verdict routes to the
/// tool the reason rests on.
///
/// A command that changes from `Routed` to `LocalOnly` (or routes somewhere
/// else) fails here, because then the reason — "the hub's tool applies the
/// fence" — has stopped being true and the judgement has to be made again.
#[test]
fn the_commands_main_added_carry_an_m1_person_fence_judgement() {
    for (command, tool, why) in M1_REVIEWED_DESKTOP_COMMANDS {
        let v = verdicts::verdict(command)
            .unwrap_or_else(|| panic!("{command} is reviewed here but has no verdict row ({why})"));
        assert_eq!(
            v.tool(),
            Some(*tool),
            "{command} no longer routes to {tool}, so its M1 person-fence judgement ({why}) rests on a tool that does not run it any more"
        );
    }
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

/// The text of one command: from its `fn <name>(` to wherever the next
/// command begins (or the end of the file, for the last one).
///
/// It ends at `#[tauri::command`, **not** at `#[tauri::command]`. The closing
/// bracket is not there: `#[tauri::command(async)]` is the other form Tauri
/// takes (`pty.rs` uses it four times), and a body that does not stop at one
/// swallows its neighbour. That is a false PASS in the direction that matters
/// — a `Routed` row whose command quietly runs the local service call would
/// still be green if any swallowed neighbour happened to contain `routed::`.
/// `a_command_body_stops_at_an_async_neighbour` is that case, in miniature.
fn command_body<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let start = src.find(&format!("fn {name}("))?;
    let rest = &src[start..];
    Some(match rest.find("#[tauri::command") {
        Some(i) => &rest[..i],
        None => rest,
    })
}

/// [`command_body`]'s one rule, on a source small enough to read.
///
/// With the old `"#[tauri::command]"` this fails: `alpha`'s body runs to the
/// end of the string, so `alpha` — which routes nothing — looks like it
/// routes, on the strength of `beta`'s call.
#[test]
fn a_command_body_stops_at_an_async_neighbour() {
    const SRC: &str = "\
#[tauri::command]
pub fn alpha(backend: State<'_, Arc<FleetBackend>>) -> Result<(), IpcError> {
    service::alpha()
}

#[tauri::command(async)]
pub fn beta(backend: State<'_, Arc<FleetBackend>>) -> Result<(), IpcError> {
    routed::beta(&backend)
}
";
    let alpha = command_body(SRC, "alpha").expect("alpha is defined");
    assert!(alpha.contains("service::alpha()"), "{alpha}");
    assert!(
        !alpha.contains("routed::"),
        "alpha's body swallowed its `(async)` neighbour, so a Routed row for a \
         command that does not route would pass on the neighbour's text:\n{alpha}"
    );
    assert!(command_body(SRC, "gamma").is_none());
}

/// **And the body agrees with the row.**
///
/// [`every_command_has_a_verdict`] proves that every command has an answer;
/// this proves the answer is the one its code gives. A row is a claim about
/// what happens at runtime, and a table nobody checks against the code is a
/// second place to be wrong.
///
/// All it needs from the source is the shape of the command's body, which is
/// why the scanner survives the table in this reduced form: three substrings
/// per command instead of a free-text search for any guard at all. A
/// `#[tauri::command]` cannot be *called* from here — it wants a live
/// `tauri::App` — so its body is read instead.
///
/// `RoutedUnless` asks for the routing call only: its refusal lives inside the
/// `routed::` function, one layer down, and is proven by
/// `repair_session_explicit_false_stays_local_only_in_remote_mode` and by the
/// message fixture.
#[test]
fn every_commands_body_does_what_its_row_says() {
    let sources: BTreeMap<&str, &str> = SOURCES.iter().copied().collect();
    let mut complaints = Vec::new();

    for (file, name) in registered_commands() {
        let src = sources
            .get(file.as_str())
            .unwrap_or_else(|| panic!("add {file} to SOURCES in tests_routing.rs"));
        let body = command_body(src, &name)
            .unwrap_or_else(|| panic!("{name} is registered but not defined in {file}"));
        let routes = body.contains("routed::");
        let refuses = body.contains(&format!("refuse_local_only(\"{name}\")"));
        let guards_at_all = body.contains("refuse_local_only(");

        let verdict = verdicts::verdict(&name)
            .unwrap_or_else(|| panic!("{name} has no row — every_command_has_a_verdict first"));
        let wrong = match verdict {
            Verdict::Routed { .. } | Verdict::RoutedUnless { .. } if !routes => {
                Some("its row routes it, but its body never reaches a `routed::` function")
            }
            Verdict::LocalOnly { .. } if !refuses => Some(
                "its row refuses it, but its body never calls \
                 `backend.refuse_local_only(\"<its own name>\")`",
            ),
            Verdict::SameInBoth { .. } if routes || guards_at_all => Some(
                "its row says it is the same in both modes, but its body routes \
                 or refuses",
            ),
            _ => None,
        };
        if let Some(why) = wrong {
            complaints.push(format!("{file}::{name}: {why}"));
        }
    }

    assert!(
        complaints.is_empty(),
        "these commands do not do what VERDICTS says they do:\n  {}\n\n\
         Fix the body, or fix the row — but they are one decision and must \
         read as one.",
        complaints.join("\n  ")
    );
}

/// A refusal is by name, so a name that the table cannot answer is a refusal
/// with the fail-closed apology in it. This makes shipping one impossible:
/// every `refuse_local_only("…")` written anywhere in the command sources must
/// name a row that has a sentence.
///
/// It covers the call sites [`every_commands_body_does_what_its_row_says`]
/// cannot see — `routed::repair_session`'s, which is not in any
/// `#[tauri::command]` body.
#[test]
fn every_refusal_names_a_command_the_table_can_refuse() {
    const CALL: &str = "refuse_local_only(\"";
    let mut refused = BTreeSet::new();
    for (file, src) in SOURCES {
        for (i, _) in src.match_indices(CALL) {
            let rest = &src[i + CALL.len()..];
            let name = &rest[..rest.find('"').expect("an unterminated command name")];
            refused.insert(name);
            let verdict = verdicts::verdict(name).unwrap_or_else(|| {
                panic!("{file} refuses {name}, which has no row in VERDICTS at all")
            });
            assert!(
                verdict.instead().is_some(),
                "{file} refuses {name}, whose row carries no sentence ({verdict:?}) — \
                 the user would get the fail-closed apology instead of a reason"
            );
        }
    }

    // Derived, not guessed: the names refused in the sources and the rows that
    // carry a sentence are the same set. A floor like `found > 70` would not
    // notice a refusal quietly disappearing, and a sentence nobody refuses
    // with is a sentence that has stopped being true.
    let can_refuse: BTreeSet<&str> = VERDICTS
        .iter()
        .filter(|(_, v)| v.instead().is_some())
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        refused,
        can_refuse,
        "the refusals written in the sources and the rows that carry a sentence \
         have drifted apart:\n  only in the sources: {:?}\n  only in the table: {:?}",
        refused.difference(&can_refuse).collect::<Vec<_>>(),
        can_refuse.difference(&refused).collect::<Vec<_>>(),
    );
}

/// The routing counterpart of
/// [`every_refusal_names_a_command_the_table_can_refuse`], and for the same
/// reason: a command routes by NAME — `HubBackend::route` looks the tool up
/// in [`VERDICTS`] — so a name the table cannot route is a call with no tool
/// to make. That miss fails closed (`HubBackend::tool_for`); this is what
/// makes it unshippable.
///
/// The two sets are asserted equal, not merely one-way. A routed row that
/// nothing routes by name would be a command still naming its tool in a
/// second literal, which is the drift the table exists to end.
///
/// The name is read across whatever whitespace rustfmt put between the paren
/// and the literal. A call whose name is not a literal at all is skipped and
/// would be invisible here — there is none today, and a command name is not
/// the sort of thing that should ever be computed.
///
/// Comment lines are blanked out first ([`strip_line_comments`]), so a
/// `route("…")` example inside a `//`/`///`/`//!` comment cannot stand in for
/// a real call the scanner should have seen deleted.
#[test]
fn every_route_names_a_command_the_table_can_route() {
    let mut routed_by_name = BTreeSet::new();
    // Stripped once per file and kept alive for the whole function: the
    // names borrowed below point into these owned strings, not into
    // `SOURCES`'s `'static` ones (stripping a `'static &str` yields an
    // owned `String` with a shorter lifetime).
    let stripped: Vec<(&str, String)> = SOURCES
        .iter()
        .map(|(file, src)| (*file, strip_line_comments(src)))
        .collect();
    for (file, src) in &stripped {
        let src = src.as_str();
        for call in ["route(", "route_text(", "route_image("] {
            for (i, _) in src.match_indices(call) {
                let Some(rest) = src[i + call.len()..].trim_start().strip_prefix('"') else {
                    continue;
                };
                let name = &rest[..rest.find('"').expect("an unterminated command name")];
                routed_by_name.insert(name);
                let verdict = verdicts::verdict(name).unwrap_or_else(|| {
                    panic!("{file} routes {name}, which has no row in VERDICTS at all")
                });
                assert!(
                    verdict.tool().is_some(),
                    "{file} routes {name}, whose row names no tool ({verdict:?}) — the \
                     call would fail closed instead of reaching the hub"
                );
            }
        }
    }

    let can_route: BTreeSet<&str> = VERDICTS
        .iter()
        .filter(|(_, v)| v.tool().is_some())
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        routed_by_name,
        can_route,
        "the routing calls written in the sources and the rows that name a tool \
         have drifted apart:\n  only in the sources: {:?}\n  only in the table: {:?}",
        routed_by_name.difference(&can_route).collect::<Vec<_>>(),
        can_route.difference(&routed_by_name).collect::<Vec<_>>(),
    );
}

/// Every `Verdict::tool()` in [`VERDICTS`] names a tool the hub actually
/// serves.
///
/// Two literals have to agree for a routed row to be right at all: the tool
/// name in the row, and the tool name in the case table that `check`'s
/// cross-check holds against the wire. Both are hand-typed, so a typo
/// repeated in both the same way is invisible to either — it would only
/// surface at runtime, when the desktop asks the hub for a tool that does
/// not exist and gets back `E_HUB_PROTOCOL` ("the hub refused the … call").
/// `fleet_core::mcp::guard::TOOL_POLICIES` is an independent third anchor:
/// the real list of tools the router serves. This makes that class of typo a
/// red test instead of a runtime surprise.
#[test]
fn every_routed_tool_is_a_tool_the_hub_serves() {
    let served: BTreeSet<&str> = fleet_core::mcp::guard::TOOL_POLICIES
        .iter()
        .map(|policy| policy.name)
        .collect();
    let missing: Vec<(&str, &str)> = VERDICTS
        .iter()
        .filter_map(|(name, v)| v.tool().map(|tool| (*name, tool)))
        .filter(|(_, tool)| !served.contains(tool))
        .collect();
    assert!(
        missing.is_empty(),
        "these VERDICTS rows name a tool that TOOL_POLICIES does not list — \
         the hub has no such tool to call:\n  {}",
        missing
            .iter()
            .map(|(name, tool)| format!("{name} -> {tool}"))
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// Blank out every line whose trimmed form starts with `//` — an ordinary
/// comment, a doc comment (`///`), or a module doc comment (`//!`) — so a
/// `route("…")` written as a comment's example cannot be mistaken by
/// [`every_route_names_a_command_the_table_can_route`] for the real call it
/// is documenting. Line-based rather than a full comment parser: every real
/// call in this codebase is `self.route(` / `hub.route(`, never split across
/// a `//` prefix, so nothing live is lost.
fn strip_line_comments(src: &str) -> String {
    src.lines()
        .map(|line| {
            if line.trim_start().starts_with("//") {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn strip_line_comments_blanks_a_route_example_in_a_doc_comment() {
    // A doc-comment example that quotes a real call, next to the call it
    // once documented having since been deleted. Before the fix, the bare
    // substring scan would still find `route("delete_worktree", …)` inside
    // the comment and count `delete_worktree` as routed, masking the
    // deletion of the real call below it.
    let src = "\
/// Example: `self.route(\"delete_worktree\", &args).await?`
//! same trick, module-doc flavour: route(\"delete_worktree\", &x)
// and a plain comment: route(\"delete_worktree\", &y)
pub async fn delete_worktree(&self) -> Result<(), IpcError> {
    // the real call used to be here; it is gone now
    Ok(())
}
";
    let stripped = strip_line_comments(src);
    assert!(
        !stripped.contains("route(\"delete_worktree\""),
        "a route(...) call inside a comment must not survive stripping:\n{stripped}"
    );
    // The real (non-comment) code is untouched.
    assert!(stripped.contains("pub async fn delete_worktree"));
}

/// Every source file the scanners above need to read.
const SOURCES: &[(&str, &str)] = &[
    // The routing calls themselves: the four reads the event bridge shares
    // with their commands live here rather than in a `routed::` function.
    ("backend/remote.rs", include_str!("remote.rs")),
    (
        "commands/account_usage.rs",
        include_str!("../commands/account_usage.rs"),
    ),
    ("commands/assets.rs", include_str!("../commands/assets.rs")),
    ("commands/cancel.rs", include_str!("../commands/cancel.rs")),
    (
        "commands/diagnostics.rs",
        include_str!("../commands/diagnostics.rs"),
    ),
    (
        "commands/downloads.rs",
        include_str!("../commands/downloads.rs"),
    ),
    (
        "commands/library.rs",
        include_str!("../commands/library.rs"),
    ),
    ("commands/editor.rs", include_str!("../commands/editor.rs")),
    ("commands/files.rs", include_str!("../commands/files.rs")),
    ("commands/health.rs", include_str!("../commands/health.rs")),
    (
        "commands/history.rs",
        include_str!("../commands/history.rs"),
    ),
    ("commands/hosts.rs", include_str!("../commands/hosts.rs")),
    ("commands/hub.rs", include_str!("../commands/hub.rs")),
    (
        "commands/local_workspaces.rs",
        include_str!("../commands/local_workspaces.rs"),
    ),
    ("commands/mcp.rs", include_str!("../commands/mcp.rs")),
    (
        "commands/missions.rs",
        include_str!("../commands/missions.rs"),
    ),
    (
        "commands/move_session.rs",
        include_str!("../commands/move_session.rs"),
    ),
    ("commands/mutate.rs", include_str!("../commands/mutate.rs")),
    ("commands/forms.rs", include_str!("../commands/forms.rs")),
    (
        "commands/debug_devices.rs",
        include_str!("../commands/debug_devices.rs"),
    ),
    (
        "commands/federation.rs",
        include_str!("../commands/federation.rs"),
    ),
    ("commands/prs.rs", include_str!("../commands/prs.rs")),
    (
        "commands/start_rules.rs",
        include_str!("../commands/start_rules.rs"),
    ),
    (
        "commands/presence.rs",
        include_str!("../commands/presence.rs"),
    ),
    (
        "commands/updates.rs",
        include_str!("../commands/updates.rs"),
    ),
    ("commands/runs.rs", include_str!("../commands/runs.rs")),
    (
        "commands/routines.rs",
        include_str!("../commands/routines.rs"),
    ),
    ("commands/pages.rs", include_str!("../commands/pages.rs")),
    (
        "commands/onboarding.rs",
        include_str!("../commands/onboarding.rs"),
    ),
    (
        "commands/operator.rs",
        include_str!("../commands/operator.rs"),
    ),
    (
        "commands/projects.rs",
        include_str!("../commands/projects.rs"),
    ),
    (
        "commands/quick_replies.rs",
        include_str!("../commands/quick_replies.rs"),
    ),
    (
        "commands/resolve_move.rs",
        include_str!("../commands/resolve_move.rs"),
    ),
    (
        "commands/sessions.rs",
        include_str!("../commands/sessions.rs"),
    ),
    ("commands/tasks.rs", include_str!("../commands/tasks.rs")),
    ("commands/tray.rs", include_str!("../commands/tray.rs")),
    ("commands/upload.rs", include_str!("../commands/upload.rs")),
    ("commands/voice.rs", include_str!("../commands/voice.rs")),
    (
        "commands/windows.rs",
        include_str!("../commands/windows.rs"),
    ),
    ("commands/work.rs", include_str!("../commands/work.rs")),
    (
        "commands/work_view.rs",
        include_str!("../commands/work_view.rs"),
    ),
    (
        "commands/trackers.rs",
        include_str!("../commands/trackers.rs"),
    ),
    ("commands/orgs.rs", include_str!("../commands/orgs.rs")),
    (
        "commands/org_devices.rs",
        include_str!("../commands/org_devices.rs"),
    ),
    (
        "commands/worktrees.rs",
        include_str!("../commands/worktrees.rs"),
    ),
    ("pty.rs", include_str!("../pty.rs")),
    ("lib.rs", include_str!("../lib.rs")),
];

/// Declarative pages P4: a resource action names a desktop command by
/// string (`fleet_core::pages::resources`), so a renamed or removed command
/// would leave a page button that fails at the click. Every one it names is
/// registered, and has a hub verdict like any other command.
#[test]
fn resource_commands_exist() {
    let registered: Vec<String> = registered_commands().into_iter().map(|(_, c)| c).collect();
    for cmd in fleet_core::pages::resources::commands() {
        assert!(
            registered.iter().any(|c| c == cmd),
            "a resource names `{cmd}`, which lib.rs does not register"
        );
        assert!(
            super::verdicts::verdict(cmd).is_some(),
            "a resource names `{cmd}`, which has no hub verdict"
        );
    }
    // A live data source is loaded by a desktop command too.
    for live in fleet_core::pages::sources::SOURCES
        .iter()
        .filter_map(|s| s.live)
    {
        assert!(
            registered.iter().any(|c| c == live.command),
            "a live source names `{}`, which lib.rs does not register",
            live.command
        );
        assert!(
            super::verdicts::verdict(live.command).is_some(),
            "a live source names `{}`, which has no hub verdict",
            live.command
        );
    }
}

/// A payload built from a real value, for the catalog answers too big to
/// write out by hand; leaked, since a [`Case`] holds a `&'static str`.
fn payload_of<T: serde::Serialize>(value: &T) -> &'static str {
    Box::leak(
        serde_json::to_string(value)
            .expect("serialisable")
            .into_boxed_str(),
    )
}

/// Every asset-catalog command that routes to `catalog_admin`: the action
/// and arguments it sends (its own argument struct, as `args`), and an answer
/// of the command's own return type. Reads and writes together, because the
/// point of the one tool is that each is the desktop command, one to one.
fn catalog_admin_cases() -> Vec<Case> {
    use commands::assets::routed as r;
    use fleet_core::service::catalog::admin::{
        AdmitArgs, CatalogNameArgs, DeleteSecretArgs, GetAssetArgs, LayerRef, LayerTemplateArgs,
        LoadArgs, ResolvePreviewArgs, SetHostHarnessesArgs, SetHostLayersArgs, SetSecretArgs,
        WriteLayerArgs,
    };
    use fleet_core::service::catalog::author::{
        self, AddResourceArgs, AssetRef, CommitPendingArgs, CreateArgs, RemoveResourceArgs,
        UpdateArgs,
    };
    use fleet_core::service::catalog::catalogs::AddCatalogArgs;
    use fleet_core::service::catalog::layer::Axis;
    use fleet_core::service::catalog::model::Kind;
    use fleet_core::service::catalog::sync::{ApplyArgs, PlanArgs};
    use fleet_core::service::catalog::{ConfigureArgs, ImportArgs};

    const CONFIG: &str =
        r#"{"repo_path":"/srv/assets","remote_url":null,"head_commit":"abc","last_loaded_at":1}"#;
    const WRITE: &str = r#"{"commit":"abc","lint":{"errors":[],"warnings":[]}}"#;
    // A minimal card, answering every `changesets` verb that returns one.
    const VIEW: &str =
        r#"{"id":3,"kind":"new","summary":"s","state":"applied","created_at":1,"items":[]}"#;
    const STATUS: &str =
        r#"{"head":"abc","dirty":0,"ahead":null,"behind":null,"has_upstream":false}"#;
    const SYNC_RUN: &str = r#"{"plan_id":"p1","started_at":1,"finished_at":2,"hosts":[]}"#;
    let asset = payload_of(&author::template(Kind::Skill, "s"));
    let detail =
        Box::leak(format!(r#"{{"asset":{asset},"previews":[],"hosts":[]}}"#).into_boxed_str());
    let layer = payload_of(&author::layer_template("core", Axis::Role));
    let host_row = payload_of(&crate::backend::contract::tests::sample_host());
    let skill = |name: &str| AssetRef {
        kind: Kind::Skill,
        name: name.into(),
    };
    let resource =
        std::env::temp_dir().join(format!("fleet-routed-resource-{}", std::process::id()));
    std::fs::write(&resource, "echo hi\n").unwrap();
    let resource = resource.to_string_lossy().into_owned();

    vec![
        (
            "catalog_config",
            "catalog_admin",
            json!({ "action": "config" }),
            CONFIG,
            Box::new(|b, s, _| block_on(r::catalog_config(b, s)).map(|_| ())),
        ),
        (
            "catalog_configure",
            "catalog_admin",
            json!({ "action": "configure",
                    "args": { "repo_path": "/srv/assets", "remote_url": "git@x:a.git" } }),
            CONFIG,
            Box::new(|b, s, _| {
                block_on(r::catalog_configure(
                    b,
                    ConfigureArgs {
                        repo_path: "/srv/assets".into(),
                        remote_url: Some("git@x:a.git".into()),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_load",
            "catalog_admin",
            json!({ "action": "load", "args": { "pull": true } }),
            r#"{"head":"abc","loaded_at":1,"asset_count":3,"problem_count":0}"#,
            Box::new(|b, s, _| block_on(r::catalog_load(b, LoadArgs { pull: true }, s)).map(|_| ())),
        ),
        (
            "catalog_get_asset",
            "catalog_admin",
            json!({ "action": "get_asset", "args": { "kind": "skill", "name": "s" } }),
            detail,
            Box::new(|b, s, _| {
                block_on(r::catalog_get_asset(
                    b,
                    GetAssetArgs {
                        kind: Kind::Skill,
                        name: "s".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_list_layers",
            "catalog_admin",
            json!({ "action": "list_layers" }),
            Box::leak(format!(r#"{{"layers":[{layer}],"hosts":[{{"host_alias":"nas","catalog_id":1,"layer_name":"core","axis":"role","position":0,"active":true}}]}}"#).into_boxed_str()),
            Box::new(|b, s, _| block_on(r::catalog_list_layers(b, s)).map(|_| ())),
        ),
        (
            "catalog_resolve_preview",
            "catalog_admin",
            json!({ "action": "resolve_preview", "args": { "host_alias": "nas" } }),
            r#"{"catalog":{"assets":[],"problems":[],"head":"abc","loaded_at":1,"layers":{"layers":{}}},"provenance":{},"excluded":{},"layered":false}"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_resolve_preview(
                    b,
                    ResolvePreviewArgs {
                        host_alias: "nas".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_propose_layers",
            "catalog_admin",
            json!({ "action": "propose_layers" }),
            r#"{"layers":[],"singletons":[{"key":"skill/s","host":"nas"}]}"#,
            Box::new(|b, s, _| block_on(r::catalog_propose_layers(b, s)).map(|_| ())),
        ),
        (
            "catalog_set_host_layers",
            "catalog_admin",
            json!({ "action": "set_host_layers",
                    "args": { "host_alias": "nas", "role": "core", "contexts": ["gpu"] } }),
            // No `catalog_id`: an older hub's answer, predating migration
            // 091, must still parse — `HostLayerRow.catalog_id` is
            // `#[serde(default)]` for exactly this.
            r#"[{"host_alias":"nas","layer_name":"core","axis":"role","position":0,"active":true}]"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_set_host_layers(
                    b,
                    SetHostLayersArgs {
                        host_alias: "nas".into(),
                        role: Some("core".into()),
                        contexts: vec!["gpu".into()],
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_set_host_harnesses",
            "catalog_admin",
            json!({ "action": "set_host_harnesses",
                    "args": { "host_alias": "nas", "harnesses": ["claude", "codex"] } }),
            host_row,
            Box::new(|b, s, _| {
                block_on(r::catalog_set_host_harnesses(
                    b,
                    SetHostHarnessesArgs {
                        host_alias: "nas".into(),
                        harnesses: Some(vec!["claude".into(), "codex".into()]),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_layer_template",
            "catalog_admin",
            json!({ "action": "layer_template", "args": { "name": "core", "axis": "role" } }),
            layer,
            Box::new(|b, _, _| {
                block_on(r::catalog_layer_template(
                    b,
                    LayerTemplateArgs {
                        name: "core".into(),
                        axis: Axis::Role,
                    },
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_write_layer",
            "catalog_admin",
            json!({ "action": "write_layer",
                    "args": { "layer": serde_json::from_str::<Value>(layer).unwrap() } }),
            r#""abc""#,
            Box::new(|b, s, _| {
                block_on(r::catalog_write_layer(
                    b,
                    WriteLayerArgs {
                        layer: author::layer_template("core", Axis::Role),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_delete_layer",
            "catalog_admin",
            json!({ "action": "delete_layer", "args": { "name": "core" } }),
            r#""abc""#,
            Box::new(|b, s, _| {
                block_on(r::catalog_delete_layer(b, LayerRef { name: "core".into() }, s))
                    .map(|_| ())
            }),
        ),
        (
            "assets_inventory",
            "catalog_admin",
            json!({ "action": "inventory" }),
            r#"[{"host_alias":"nas","harness":"claude","kind":"skill","name":"s","state":"in_sync","scanned_at":1,"managed":true}]"#,
            Box::new(|b, s, _| block_on(r::assets_inventory(b, s)).map(|_| ())),
        ),
        // Task 6: import works from any host over SSH, not just `local`.
        (
            "catalog_import_host",
            "catalog_admin",
            json!({ "action": "import_host",
                    "args": { "host_alias": "oci", "dry_run": true, "only": [] } }),
            r#"{"created":[],"problems":[],"warnings":[],"flagged_secrets":[],"dry_run":true}"#,
            Box::new(|b, s, h| {
                block_on(r::catalog_import_host(
                    b,
                    ImportArgs {
                        host_alias: "oci".into(),
                        dry_run: true,
                        only: vec![],
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_plan_sync",
            "catalog_admin",
            json!({ "action": "plan_sync",
                    "args": { "host_alias": "nas", "kind": "skill", "name": "s", "allow_unlayered": false } }),
            r#"{"id":"p1","computed_at":1,"hosts":[],"counts":{}}"#,
            Box::new(|b, s, h| {
                block_on(r::catalog_plan_sync(
                    b,
                    PlanArgs {
                        host_alias: Some("nas".into()),
                        kind: Some(Kind::Skill),
                        name: Some("s".into()),
                        allow_unlayered: false,
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_apply_sync",
            "catalog_admin",
            json!({ "action": "apply_sync",
                    "args": { "plan_id": "p1", "force_partial": true, "call_id": 9 } }),
            SYNC_RUN,
            Box::new(|b, s, h| {
                block_on(r::catalog_apply_sync(
                    b,
                    ApplyArgs {
                        plan_id: "p1".into(),
                        force_partial: true,
                        call_id: Some(9),
                    },
                    s,
                    h,
                    &fleet_core::cancel::CancellationRegistry::new(),
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_last_sync",
            "catalog_admin",
            json!({ "action": "last_sync" }),
            SYNC_RUN,
            Box::new(|b, s, _| block_on(r::catalog_last_sync(b, s)).map(|_| ())),
        ),
        (
            "catalog_list_secrets",
            "catalog_admin",
            json!({ "action": "list_secrets" }),
            r#"[{"name":"API_TOKEN","host_alias":null,"updated_at":1}]"#,
            Box::new(|b, s, _| block_on(r::catalog_list_secrets(b, s)).map(|_| ())),
        ),
        (
            "catalog_set_secret",
            "catalog_admin",
            json!({ "action": "set_secret",
                    "args": { "name": "API_TOKEN", "host_alias": "nas", "value": "v" } }),
            "null",
            Box::new(|b, s, _| {
                block_on(r::catalog_set_secret(
                    b,
                    SetSecretArgs {
                        name: "API_TOKEN".into(),
                        host_alias: Some("nas".into()),
                        value: "v".into(),
                    },
                    s,
                ))
            }),
        ),
        (
            "catalog_delete_secret",
            "catalog_admin",
            json!({ "action": "delete_secret", "args": { "name": "API_TOKEN", "host_alias": null } }),
            "true",
            Box::new(|b, s, _| {
                block_on(r::catalog_delete_secret(
                    b,
                    DeleteSecretArgs {
                        name: "API_TOKEN".into(),
                        host_alias: None,
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_create_asset",
            "catalog_admin",
            json!({ "action": "create_asset",
                    "args": { "kind": "skill", "name": "fresh", "duplicate_from": "s" } }),
            WRITE,
            Box::new(|b, s, _| {
                block_on(r::catalog_create_asset(
                    b,
                    CreateArgs {
                        kind: Kind::Skill,
                        name: "fresh".into(),
                        duplicate_from: Some("s".into()),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_update_asset",
            "catalog_admin",
            json!({ "action": "update_asset",
                    "args": { "asset": serde_json::from_str::<Value>(asset).unwrap() } }),
            WRITE,
            Box::new(|b, s, _| {
                block_on(r::catalog_update_asset(
                    b,
                    UpdateArgs {
                        asset: author::template(Kind::Skill, "s"),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_delete_asset",
            "catalog_admin",
            json!({ "action": "delete_asset", "args": { "kind": "skill", "name": "s" } }),
            r#""abc""#,
            Box::new(move |b, s, _| block_on(r::catalog_delete_asset(b, skill("s"), s)).map(|_| ())),
        ),
        // The file is read on this side; its bytes are what the hub gets.
        (
            "catalog_add_resource",
            "catalog_admin",
            json!({ "action": "add_resource_bytes",
                    "args": { "kind": "skill", "name": "s", "rel_path": "resources/run.sh",
                              "bytes": "ZWNobyBoaQo=" } }),
            WRITE,
            Box::new(move |b, s, _| {
                block_on(r::catalog_add_resource(
                    b,
                    AddResourceArgs {
                        kind: Kind::Skill,
                        name: "s".into(),
                        local_path: resource.clone(),
                        rel_path: Some("resources/run.sh".into()),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_remove_resource",
            "catalog_admin",
            json!({ "action": "remove_resource",
                    "args": { "kind": "skill", "name": "s", "rel_path": "resources/run.sh" } }),
            WRITE,
            Box::new(|b, s, _| {
                block_on(r::catalog_remove_resource(
                    b,
                    RemoveResourceArgs {
                        kind: Kind::Skill,
                        name: "s".into(),
                        rel_path: "resources/run.sh".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_lint_asset",
            "catalog_admin",
            json!({ "action": "lint_asset", "args": { "kind": "skill", "name": "s" } }),
            r#"{"errors":[],"warnings":[{"field":"description","message":"short"}]}"#,
            Box::new(move |b, s, _| block_on(r::catalog_lint_asset(b, skill("s"), s)).map(|_| ())),
        ),
        (
            "catalog_lint_all",
            "catalog_admin",
            json!({ "action": "lint_all" }),
            r#"{"assets":[],"problems":[],"errors":0,"warnings":0}"#,
            Box::new(|b, s, _| block_on(r::catalog_lint_all(b, s)).map(|_| ())),
        ),
        (
            "catalog_commit_pending",
            "catalog_admin",
            json!({ "action": "commit_pending", "args": { "message": "catalog: tidy" } }),
            r#""abc""#,
            Box::new(|b, s, _| {
                block_on(r::catalog_commit_pending(
                    b,
                    CommitPendingArgs {
                        message: Some("catalog: tidy".into()),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_push",
            "catalog_admin",
            json!({ "action": "push" }),
            STATUS,
            Box::new(|b, s, _| block_on(r::catalog_push(b, s)).map(|_| ())),
        ),
        (
            "catalog_repo_status",
            "catalog_admin",
            json!({ "action": "repo_status" }),
            STATUS,
            Box::new(|b, s, _| block_on(r::catalog_repo_status(b, s)).map(|_| ())),
        ),
        (
            "catalog_template",
            "catalog_admin",
            json!({ "action": "template", "args": { "kind": "skill", "name": "s" } }),
            asset,
            Box::new(move |b, _, _| block_on(r::catalog_template(b, skill("s"))).map(|_| ())),
        ),
        // Assets M5 (R13): the workspace's reads. A named catalog travels as
        // the tool's own top-level `catalog`, never inside `args`.
        (
            "catalog_list_catalogs",
            "catalog_admin",
            json!({ "action": "list_catalogs" }),
            "[]",
            Box::new(|b, s, _| block_on(r::catalog_list_catalogs(b, s)).map(|_| ())),
        ),
        (
            "catalog_list_changesets",
            "changesets",
            json!({ "action": "list" }),
            r#"[{"id":3,"kind":"new","summary":"New on oci: skill/w → core","state":"proposed","created_at":1,"groups":{"core":1},"pending":1,"undoable":false}]"#,
            Box::new(|b, s, _| block_on(r::catalog_list_changesets(b, s)).map(|_| ())),
        ),
        // Assets M6 (R8): the card verbs — all `changesets`.
        (
            "catalog_get_changeset",
            "changesets",
            json!({ "action": "list", "id": 3 }),
            VIEW,
            Box::new(|b, s, _| {
                block_on(r::catalog_get_changeset(b, commands::assets::ChangesetIdArgs { id: 3 }, s))
                .map(|_| ())
            }),
        ),
        (
            "catalog_apply_changeset",
            "changesets",
            json!({ "action": "apply", "id": 3, "positions": [0, 2] }),
            VIEW,
            Box::new(|b, s, h| {
                block_on(r::catalog_apply_changeset(
                    b,
                    commands::assets::ApplyChangesetArgs { id: 3, positions: Some(vec![0, 2]) },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_apply_changeset",
            "changesets",
            json!({ "action": "apply", "id": 3 }),
            VIEW,
            Box::new(|b, s, h| {
                block_on(r::catalog_apply_changeset(
                    b,
                    commands::assets::ApplyChangesetArgs { id: 3, positions: None },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_undo_changeset",
            "changesets",
            json!({ "action": "undo", "id": 3 }),
            VIEW,
            Box::new(|b, s, _| {
                block_on(r::catalog_undo_changeset(b, commands::assets::ChangesetIdArgs { id: 3 }, s))
                .map(|_| ())
            }),
        ),
        (
            "catalog_dismiss_changeset",
            "changesets",
            json!({ "action": "dismiss", "id": 3 }),
            VIEW,
            Box::new(|b, s, _| {
                block_on(r::catalog_dismiss_changeset(b, commands::assets::ChangesetIdArgs { id: 3 }, s))
                .map(|_| ())
            }),
        ),
        (
            "catalog_reject_changeset_items",
            "changesets",
            json!({ "action": "reject_item", "id": 3, "positions": [1] }),
            VIEW,
            Box::new(|b, s, _| {
                block_on(r::catalog_reject_changeset_items(
                    b,
                    commands::assets::RejectItemsArgs { id: 3, positions: vec![1] },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_propose_changesets",
            "changesets",
            json!({ "action": "propose" }),
            "[]",
            Box::new(|b, s, _| {
                block_on(r::catalog_propose_changesets(b, s))
                .map(|_| ())
            }),
        ),
        (
            "catalog_propose_layer_change",
            "changesets",
            json!({ "action": "propose_layer", "change": { "op": "rename", "layer": "core", "to": "base" } }),
            VIEW,
            Box::new(|b, s, _| {
                block_on(r::catalog_propose_layer_change(
                    b,
                    commands::assets::LayerChangeArgs {
                        change: fleet_core::service::catalog::changesets::LayerChange::Rename {
                            catalog: None,
                            layer: "core".into(),
                            to: "base".into(),
                        },
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_repo_status_in",
            "catalog_admin",
            json!({ "action": "repo_status", "catalog": "acme" }),
            STATUS,
            Box::new(|b, s, _| {
                block_on(r::catalog_repo_status_in(
                    b,
                    CatalogNameArgs {
                        name: "acme".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_asset_history",
            "catalog_admin",
            json!({ "action": "asset_history",
                    "args": { "kind": "skill", "name": "s" },
                    "catalog": "acme" }),
            r#"[{"sha":"abc","at":1,"author":"a","subject":"edit s"}]"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_asset_history(
                    b,
                    commands::assets::AssetHistoryArgs {
                        kind: Kind::Skill,
                        name: "s".into(),
                        catalog: Some("acme".into()),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // Personal (named or not) sends no `catalog`, exactly the pre-M5 shape.
        (
            "catalog_asset_history",
            "catalog_admin",
            json!({ "action": "asset_history", "args": { "kind": "skill", "name": "s" } }),
            "[]",
            Box::new(|b, s, _| {
                block_on(r::catalog_asset_history(
                    b,
                    commands::assets::AssetHistoryArgs {
                        kind: Kind::Skill,
                        name: "s".into(),
                        catalog: Some("personal".into()),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // Assets M6 (R9): the catalog set. The name travels in `args`, never
        // as the tool's top-level `catalog` (the hub's `touches` reads it
        // there).
        (
            "catalog_add_catalog",
            "catalog_admin",
            json!({ "action": "add_catalog",
                    "args": { "name": "acme", "repo_path": "/r", "remote_url": null, "org": "Acme" } }),
            r#"{"id":2,"name":"acme","org_id":1,"org":"Acme","repo_path":"/r","remote_url":null,"head_commit":null,"last_loaded_at":null,"state":"not_loaded","asset_count":0}"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_add_catalog(
                    b,
                    AddCatalogArgs {
                        name: "acme".into(),
                        repo_path: "/r".into(),
                        remote_url: None,
                        org: Some("Acme".into()),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_remove_catalog",
            "catalog_admin",
            json!({ "action": "remove_catalog", "args": { "name": "acme" } }),
            r#"{"id":2,"name":"acme","layer_rows":0,"admissions":1,"grants":0,"cards":0}"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_remove_catalog(
                    b,
                    CatalogNameArgs {
                        name: "acme".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_admit_catalog",
            "catalog_admin",
            json!({ "action": "admit_catalog", "args": { "host_alias": "mefistos", "catalog": "acme" } }),
            r#"["acme"]"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_admit_catalog(
                    b,
                    AdmitArgs {
                        host_alias: "mefistos".into(),
                        catalog: "acme".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_unadmit_catalog",
            "catalog_admin",
            json!({ "action": "unadmit_catalog", "args": { "host_alias": "mefistos", "catalog": "acme" } }),
            "[]",
            Box::new(|b, s, _| {
                block_on(r::catalog_unadmit_catalog(
                    b,
                    AdmitArgs {
                        host_alias: "mefistos".into(),
                        catalog: "acme".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // One catalog's layers: a named catalog is the tool's top-level
        // `catalog`; personal sends none, exactly `catalog_list_layers`.
        (
            "catalog_list_layers_in",
            "catalog_admin",
            json!({ "action": "list_layers", "catalog": "acme" }),
            r#"{"layers":[],"hosts":[]}"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_list_layers_in(
                    b,
                    CatalogNameArgs {
                        name: "acme".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_list_layers_in",
            "catalog_admin",
            json!({ "action": "list_layers" }),
            r#"{"layers":[],"hosts":[]}"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_list_layers_in(
                    b,
                    CatalogNameArgs {
                        name: "personal".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        // One host's provenance is the MCP `resolve_preview` tool, and its
        // answer is the summary view: `withheld` a list of `[kind, name]`
        // pairs, `refused` a list of objects, `held_back` a map.
        (
            "catalog_host_provenance",
            "resolve_preview",
            json!({ "host_alias": "oci" }),
            r#"{"provenance":{"skill/w":{"introduced_by":"core","overridden_by":[],"catalog":"personal"}},"excluded":{},"refused":[{"kind":"skill","name":"x","reason":"private","catalog":"personal"}],"withheld":[["skill","p"]],"held_back":{"acme":"not loaded"},"assets":[{"kind":"skill","name":"w","version":"1"}]}"#,
            Box::new(|b, s, _| {
                block_on(r::catalog_host_provenance(
                    b,
                    ResolvePreviewArgs {
                        host_alias: "oci".into(),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
        (
            "catalog_drift_diff",
            "catalog_admin",
            json!({ "action": "drift_diff",
                    "args": { "host_alias": "oci", "kind": "skill", "name": "w" },
                    "catalog": "acme" }),
            r#"{"host_alias":"oci","harness":"claude","files":[]}"#,
            Box::new(|b, s, h| {
                block_on(r::catalog_drift_diff(
                    b,
                    commands::assets::DriftDiffCmdArgs {
                        host_alias: "oci".into(),
                        kind: Kind::Skill,
                        name: "w".into(),
                        harness: None,
                        catalog: Some("acme".into()),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        // Personal (named or not) sends no `catalog`.
        (
            "catalog_drift_diff",
            "catalog_admin",
            json!({ "action": "drift_diff",
                    "args": { "host_alias": "oci", "kind": "skill", "name": "w", "harness": "codex" } }),
            r#"{"host_alias":"oci","harness":"codex","files":[{"path":"~/.codex/skills/w/SKILL.md","catalog":"a","host":"b"}]}"#,
            Box::new(|b, s, h| {
                block_on(r::catalog_drift_diff(
                    b,
                    commands::assets::DriftDiffCmdArgs {
                        host_alias: "oci".into(),
                        kind: Kind::Skill,
                        name: "w".into(),
                        harness: Some("codex".into()),
                        catalog: Some("personal".into()),
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
    ]
}

/// `save_download` on a hub reads the row through its VERDICTS row's tool
/// first, and refuses one that is not ready without asking where to save.
#[test]
fn a_hub_download_is_checked_through_list_downloads_before_a_byte_moves() {
    let fake = Fake::answering(
        r#"{"downloads":[{"id":7,"at":1,"host_alias":"trn","path":"/w/a.pdf","name":"a.pdf","size":3,"state":"fetching","source":"agent"}],"total_bytes":3,"max_total_bytes":10,"max_file_bytes":5}"#,
    );
    let (_dir, st) = store();
    let asked = std::sync::atomic::AtomicBool::new(false);
    let got = block_on(commands::downloads::routed::save_download(
        &remote_backend(&fake),
        7,
        &st,
        |_| {
            asked.store(true, std::sync::atomic::Ordering::SeqCst);
            async { None }
        },
    ));
    let (tool, args) = fake.only_call();
    assert_eq!(
        verdicts::verdict("save_download").and_then(Verdict::tool),
        Some(tool.as_str())
    );
    assert_eq!(args, json!({ "limit": 200 }));
    let e = got.expect_err("a file still copying is not saved");
    assert_eq!(e.code, codes::E_NOTFOUND);
    assert!(!asked.load(std::sync::atomic::Ordering::SeqCst));
}

/// 11.6: the one routed command whose answer is an image block rather than
/// the tool's JSON. The caption is the first text block, the image the first
/// image block, and the row's tool is the one the request carried.
#[test]
fn a_hub_screenshot_comes_back_as_the_image_block_it_answered() {
    let fake = Arc::new(Fake {
        body: format!(
            "event: message\ndata: {}\n\n",
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": { "content": [
                    { "type": "text", "text": "Pixel 8 on mac: 3 bytes" },
                    { "type": "image", "data": "AAEC", "mimeType": "image/png" },
                ] },
            })
        ),
        seen: Mutex::new(Vec::new()),
    });
    let (_dir, st) = store();
    let shot = block_on(commands::debug_devices::routed::debug_device_screenshot(
        &remote_backend(&fake),
        &st,
        &ssh(),
        commands::debug_devices::DebugDeviceArgs { id: 3 },
    ))
    .expect("the image block");
    let (tool, args) = fake.only_call();
    assert_eq!(args, json!({ "action": "screenshot", "device": "3" }));
    assert_eq!(
        verdicts::verdict("debug_device_screenshot").and_then(Verdict::tool),
        Some(tool.as_str())
    );
    assert_eq!(
        (
            shot.caption.as_str(),
            shot.mime.as_str(),
            shot.data.as_str()
        ),
        ("Pixel 8 on mac: 3 bytes", "image/png", "AAEC")
    );

    // A text-only answer is not an image: an error, never an empty picture.
    let fake = Fake::answering("{}");
    let err = block_on(commands::debug_devices::routed::debug_device_screenshot(
        &remote_backend(&fake),
        &st,
        &ssh(),
        commands::debug_devices::DebugDeviceArgs { id: 3 },
    ))
    .expect_err("no image block");
    assert_eq!(err.code, fleet_core::ipc_error::codes::E_PARSE);
}

/// Update-channel design §6.5: the one thing a skewed hub still answers is
/// what to update to. `/update/check` and `/update/report` — and nothing
/// else — bypass the wire-contract gate every routed command refuses at.
#[test]
fn update_routes_are_the_only_contract_exemption() {
    assert_eq!(
        remote::HubBackend::UPDATE_PATHS,
        ["/update/check", "/update/report"]
    );
    for state in [
        connection::HubConnection::HubTooOld {
            hub_contract: 1,
            min_contract: 3,
        },
        connection::HubConnection::HubTooNew {
            hub_contract: 9,
            max_contract: 1,
        },
    ] {
        let fake = Fake::answering("{}");
        let backend = skewed_backend(&fake, state);
        let hub = backend.hub().expect("a remote backend");
        block_on(hub.post_update("/update/check", r#"{"update_proto":1}"#.into()))
            .expect("a skewed hub is still asked what to update to");
        assert_eq!(
            fake.seen.lock().unwrap().as_slice(),
            [r#"{"update_proto":1}"#.to_string()]
        );
        let err = block_on(hub.post_update("/mcp", "{}".into())).unwrap_err();
        assert_eq!(
            err.code,
            codes::E_INTERNAL,
            "only the update routes: {err:?}"
        );
        // The artifact mirror: its own route only, never another GET.
        let dest = std::env::temp_dir().join("fleet-not-written");
        for path in [
            "/downloads/3",
            "/update/artifact/short",
            "/update/artifact/../../x",
        ] {
            let err = block_on(hub.fetch_update_artifact(path, &dest, 10)).unwrap_err();
            assert_eq!(err.code, codes::E_INTERNAL, "{path}: {err:?}");
        }
    }
}
