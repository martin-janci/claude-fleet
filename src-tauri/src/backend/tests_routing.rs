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
fn store() -> (tempfile::TempDir, Arc<Mutex<Store>>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
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
const SESSION_PAYLOAD: &str = r#"{"id":42,"tmux_name":"from-the-hub","host_alias":"hetzner","created_at":1,"last_activity_at":2,"status":"running","kind":"tmux","turn_seq":0,"tags":[]}"#;
/// `transport` is required on the wire (migration 034): it is a `String`,
/// not an `Option`, so `ok_json_compact` never strips it and a row without
/// it does not parse.
const HOST_PAYLOAD: &str =
    r#"{"alias":"trn","reachable":true,"hidden":false,"provisioned":true,"transport":"ssh"}"#;
const TASK_PAYLOAD: &str = r#"{"id":11,"state":"cancelled","created_at":1}"#;
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
const ROUTED_WITHOUT_A_CASE: &[(&str, &str)] = &[(
    "health_check",
    "health_is_the_hubs_fleet_not_this_apps_empty_database asserts its empty \
     arguments and cross-checks its VERDICTS row against the tool the request \
     carried, the same way check does; it is not a case because it also needs \
     a seeded local store to prove the answer is not the local one",
)];

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
    use fleet_core::service::repo::SessionIdArgs;
    use fleet_core::service::repo_read::{
        RepoCommitArgs, RepoCommitDiffArgs, RepoFileArgs, RepoLogArgs,
    };
    use fleet_core::service::sessions::RelatedSessionsArgs;
    use fleet_core::service::worktrees::{ListHostWorktreesArgs, ListWorktreesArgs};

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
            "related_sessions",
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
            "list_projects",
            "list_projects",
            json!({ "summary": false }),
            "[]",
            Box::new(|b, s, _| {
                block_on(commands::projects::routed::list_projects(b, s)).map(|_| ())
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
            // The tool's own defaults (slim rows, one page, a {total,
            // worktrees} envelope) are shaped for an agent; the desktop draws
            // the whole tree, so it asks for full rows and limit 0 (no cap).
            "list_worktrees",
            "list_worktrees",
            json!({ "project_id": 4, "summary": false, "limit": 0 }),
            r#"{"total":0,"worktrees":[]}"#,
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
        // The other shape of the same argument: an omitted project filter is
        // sent as an explicit `null`, not left off the object. Which of the
        // two the hub sees is the difference between "every worktree" and a
        // parameter it never bound, so both shapes are pinned rather than
        // one.
        (
            "list_worktrees",
            "list_worktrees",
            json!({ "project_id": null, "summary": false, "limit": 0 }),
            r#"{"total":0,"worktrees":[]}"#,
            Box::new(|b, s, _| {
                block_on(commands::worktrees::routed::list_worktrees(
                    b,
                    ListWorktreesArgs { project_id: None },
                    s,
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
            "work_scopes",
            "work",
            json!({ "session_id": null, "key": null, "action": "scopes" }),
            r#"[{"id":1,"label":"Company A","session_count":2,"needs_you":1}]"#,
            Box::new(|b, s, _| block_on(commands::orgs::routed::work_scopes(b, s)).map(|_| ())),
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
                    "view": "mine", "query": "login", "limit": 20 }),
            r#"[{"id":3,"source":"jira","key":"ABC-1","title":"Login","status_category":"todo","created_at":1,"updated_at":1}]"#,
            Box::new(|b, s, _| {
                block_on(commands::trackers::routed::work_tickets(
                    b,
                    commands::trackers::WorkTicketsArgs {
                        tracker_id: None,
                        view: Some("mine".into()),
                        query: Some("login".into()),
                        limit: Some(20),
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
    use fleet_core::service::move_session::resolve::{ResolveMoveAction, ResolveMoveArgs};
    use fleet_core::service::move_session::MoveSessionArgs;
    use fleet_core::service::safe_kill::SafeKillSessionArgs;
    use fleet_core::service::sessions::{
        DiscoverLostSessionsArgs, DismissGhostSessionArgs, KillSessionArgs, NewSessionArgs,
        RecreateSessionArgs, RenameSessionArgs, RestartSessionArgs, RestoreHostSessionsArgs,
        SendPromptArgs, SetFriendlyNameArgs, SpawnReviewArgs,
    };
    use fleet_core::service::worktrees::DeleteWorktreeArgs;

    vec![
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
                    },
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
                    },
                    s,
                    h,
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
                    },
                    s,
                    h,
                ))
                .map(|_| ())
            }),
        ),
        (
            "delete_worktree",
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
            "cancel_task",
            json!({ "task_id": 11 }),
            TASK_PAYLOAD,
            Box::new(|b, s, _| {
                block_on(commands::tasks::routed::cancel_task(b, 11, s)).map(|_| ())
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
            },
            &st,
        )),
        block_on(commands::work::routed::unlink_session_work(
            &local,
            commands::work::UnlinkSessionWorkArgs {
                session_id: 99,
                link_id: 1,
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
        .refuse_local_only("catalog_push")
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
        .refuse_local_only("catalog_push")
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
        let said = reason(command);
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
        let said = reason(command);
        assert!(said.contains("master"), "{command}: {said}");
    }

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
    assert!(
        actual.lines().count() > 70,
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
        for call in ["route(", "route_text("] {
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
        "commands/operator.rs",
        include_str!("../commands/operator.rs"),
    ),
    (
        "commands/projects.rs",
        include_str!("../commands/projects.rs"),
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
    ("commands/upload.rs", include_str!("../commands/upload.rs")),
    ("commands/work.rs", include_str!("../commands/work.rs")),
    (
        "commands/trackers.rs",
        include_str!("../commands/trackers.rs"),
    ),
    ("commands/orgs.rs", include_str!("../commands/orgs.rs")),
    (
        "commands/worktrees.rs",
        include_str!("../commands/worktrees.rs"),
    ),
    ("pty.rs", include_str!("../pty.rs")),
    ("lib.rs", include_str!("../lib.rs")),
];
