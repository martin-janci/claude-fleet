//! The org isolation matrix (work graph M5.3) — the acceptance gate of the
//! boundary.
//!
//! Every `work` / `work_link` / `work_admin` action — enumerated from
//! `WORK_ACTIONS`, `WORK_LINK_ACTIONS` and `AdminAction::NAMES`, so a new
//! action without a row here fails `every_action_has_a_matrix_row` — runs
//! against six callers (master, client full, client readonly, a host in org
//! A, a host in org B, a host in no org), with `isolate_sessions` off and on
//! for org B. Each call goes through what `call_tool` does around a tool:
//! the mode and admin gates before it, the org redaction after it.
//!
//! Two kinds of check run on every row:
//!
//! * **No leak.** Whatever a host-bound caller gets back — a result or an
//!   error sentence — never contains another org's markers (title, key,
//!   journal and description text). A host in no org sees neither org's.
//! * **The row's own expectation**: who gets what, which error, and that an
//!   id of another org answers exactly as an id that does not exist.

use super::*;
use crate::service::orgs::OrgScope;
use crate::service::trackers::admin::AdminAction;
use crate::service::work::{WORK_ACTIONS, WORK_LINK_ACTIONS};
use crate::store::{TrackerItemWrite, WorkTarget};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const A_MARKERS: &[&str] = &["SECRET-A", "Alpha", "A-1"];
const B_MARKERS: &[&str] = &["SECRET-B", "Bravo", "B-1", "B-2", "B-3"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    Master,
    ClientFull,
    ClientReadonly,
    HostA,
    HostB,
    HostNone,
}

const EVERYONE: &[Who] = &[
    Who::Master,
    Who::ClientFull,
    Who::ClientReadonly,
    Who::HostA,
    Who::HostB,
    Who::HostNone,
];

impl Who {
    fn caller(self) -> Caller {
        let host = |h: &str| Caller {
            host_alias: Some(h.into()),
            client: None,
            mode: TokenMode::Full,
        };
        let client = |mode| Caller {
            host_alias: None,
            client: Some(crate::mcp::auth::ClientRef {
                id: 7,
                name: "phone".into(),
                trusted: false,
            }),
            mode,
        };
        match self {
            Who::Master => Caller::master(),
            Who::ClientFull => client(TokenMode::Full),
            Who::ClientReadonly => client(TokenMode::Readonly),
            Who::HostA => host("h-a"),
            Who::HostB => host("h-b"),
            Who::HostNone => host("h-n"),
        }
    }

    fn is_host(self) -> bool {
        matches!(self, Who::HostA | Who::HostB | Who::HostNone)
    }

    /// Markers this caller must never read.
    fn forbidden_markers(self) -> Vec<&'static str> {
        match self {
            Who::HostA => B_MARKERS.to_vec(),
            Who::HostB => A_MARKERS.to_vec(),
            Who::HostNone => A_MARKERS.iter().chain(B_MARKERS).copied().collect(),
            _ => Vec::new(),
        }
    }
}

struct Fx {
    t: FleetTools,
    org_b: i64,
    /// Sessions: A's on h-a, B's on h-b, an unassigned one on h-n, and a
    /// second h-a session a person force-linked to B's ticket.
    s_a: i64,
    s_b: i64,
    s_n: i64,
    s_x: i64,
    item_a: i64,
    item_b: i64,
    tracker_a: i64,
    tracker_b: i64,
    pid_beta: i64,
    /// s_b's live link to BB-1, and s_x's forced link to BB-1.
    link_b: i64,
    link_x: i64,
}

fn item(s: &Store, tracker: i64, ext: &str, key: &str, title: &str, desc: &str) -> i64 {
    s.upsert_tracker_item(
        tracker,
        &TrackerItemWrite {
            external_id: ext.into(),
            key: Some(key.into()),
            title: title.into(),
            status_name: "To Do".into(),
            status_category: "todo".into(),
            description: Some(desc.into()),
            ..Default::default()
        },
    )
    .unwrap()
    .id
}

fn fixture(isolate_b: bool) -> Fx {
    let s = Store::open_in_memory().unwrap();
    for h in ["h-a", "h-b", "h-n"] {
        s.upsert_host(h).unwrap();
    }
    s.conn_for_test()
        .execute("UPDATE hosts SET reachable = 1", [])
        .unwrap();
    let a = s.add_org("Company A", Some("#f00"), false).unwrap();
    let b = s.add_org("Company B", Some("#00f"), isolate_b).unwrap();
    for (org, owner) in [(a.id, "acme"), (b.id, "beta")] {
        s.add_org_rule(crate::store::OrgRuleRow {
            org_id: org,
            owner: Some(owner.into()),
            ..Default::default()
        })
        .unwrap();
    }
    s.set_host_org("h-a", Some(a.id)).unwrap();
    s.set_host_org("h-b", Some(b.id)).unwrap();
    let pid_acme = s.upsert_project("acme", "api", "/src/acme").unwrap();
    let pid_beta = s.upsert_project("beta", "web", "/src/beta").unwrap();
    let ta = s
        .add_tracker("jira", "A Jira", "https://alpha.atlassian.net")
        .unwrap();
    let tb = s
        .add_tracker("jira", "B Jira", "https://bravo.atlassian.net")
        .unwrap();
    s.set_tracker_org(ta.id, Some(a.id)).unwrap();
    s.set_tracker_org(tb.id, Some(b.id)).unwrap();
    let item_a = item(
        &s,
        ta.id,
        "1",
        "AA-1",
        "Alpha login",
        "SECRET-A description",
    );
    let item_b = item(
        &s,
        tb.id,
        "1",
        "BB-1",
        "Bravo payroll",
        "SECRET-B description",
    );
    let item_b2 = item(&s, tb.id, "2", "BB-2", "Bravo two", "SECRET-B two");
    let item_b3 = item(&s, tb.id, "3", "BB-3", "Bravo three", "SECRET-B three");
    let _ = item_b2;

    let sess = |name: &str, host: &str, pid: Option<i64>, conv: &str| {
        let id = s
            .upsert_session(name, host, pid, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, conv).unwrap();
        id
    };
    let s_a = sess("s-a", "h-a", Some(pid_acme), "conv-a");
    let s_b = sess("s-b", "h-b", Some(pid_beta), "conv-b");
    let s_n = sess("s-n", "h-n", None, "conv-n");
    let s_x = sess("s-x", "h-a", Some(pid_acme), "conv-x");
    s.link_session_work(s_a, WorkTarget::Item(item_a), "manual")
        .unwrap();
    let link_b = s
        .link_session_work(s_b, WorkTarget::Item(item_b), "manual")
        .unwrap()
        .id;
    s.link_session_work(s_n, WorkTarget::Key("LOC-1"), "manual")
        .unwrap();
    // A person forced B's ticket onto an A session (the store takes it; the
    // service would have asked for force_cross_org).
    let link_x = s
        .link_session_work(s_x, WorkTarget::Item(item_b), "manual")
        .unwrap()
        .id;
    for (conv, body) in [
        ("conv-a", "SECRET-A progress"),
        ("conv-b", "SECRET-B progress"),
    ] {
        s.append_journal(Some(conv), None, "progress", "hook", Some(body), None)
            .unwrap();
    }
    // Past work of B: an ended session with its journal.
    let old = sess("s-b-old", "h-b", Some(pid_beta), "conv-b3");
    s.link_session_work(old, WorkTarget::Item(item_b3), "manual")
        .unwrap();
    s.append_journal(
        Some("conv-b3"),
        None,
        "progress",
        "hook",
        Some("SECRET-B old progress"),
        None,
    )
    .unwrap();
    s.delete_session(old).unwrap();
    let t = FleetTools::new(
        Arc::new(Mutex::new(s)),
        Arc::new(SshClient::new()),
        CancellationRegistry::new(),
        Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
        McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {})),
    );
    Fx {
        t,
        org_b: b.id,
        s_a,
        s_b,
        s_n,
        s_x,
        item_a,
        item_b,
        tracker_a: ta.id,
        tracker_b: tb.id,
        pid_beta,
        link_b,
        link_x,
    }
}

/// What a call answered: the result text, or the error message (which starts
/// with its code).
type Answer = Result<String, String>;

fn code(a: &Answer) -> &str {
    match a {
        Ok(_) => "OK",
        Err(m) => m.split(':').next().unwrap_or(m),
    }
}

fn text(a: &Answer) -> &str {
    match a {
        Ok(t) | Err(t) => t,
    }
}

/// One call the way `call_tool` makes it: the mode and admin gates, the
/// tool, then (for a per-host token) the org redaction of the result.
async fn call(fx: &Fx, who: Who, tool: &str, args: Value) -> Answer {
    let c = who.caller();
    enforce_mode(&c, tool)
        .and_then(|()| enforce_admin(&c, tool))
        .map_err(|e| e.message.to_string())?;
    let ext = Extension(c.clone());
    let r = match tool {
        "work" => {
            fx.t.work(ext, Parameters(serde_json::from_value(args).unwrap()))
                .await
        }
        "work_link" => {
            fx.t.work_link(ext, Parameters(serde_json::from_value(args).unwrap()))
                .await
        }
        "work_admin" => {
            fx.t.work_admin(ext, Parameters(serde_json::from_value(args).unwrap()))
                .await
        }
        "list_sessions" => {
            fx.t.list_sessions(ext, Parameters(serde_json::from_value(args).unwrap()))
                .await
        }
        "peer_status" => {
            fx.t.peer_status(ext, Parameters(serde_json::from_value(args).unwrap()))
                .await
        }
        "related_sessions" => {
            fx.t.related_sessions(ext, Parameters(serde_json::from_value(args).unwrap()))
                .await
        }
        "send_message" => {
            fx.t.send_message(ext, Parameters(serde_json::from_value(args).unwrap()))
                .await
        }
        "session_history" => {
            fx.t.session_history(ext, Parameters(serde_json::from_value(args).unwrap()))
                .await
        }
        "repo_changes" => {
            fx.t.repo_changes(ext, Parameters(serde_json::from_value(args).unwrap()))
                .await
        }
        "whoami" => {
            fx.t.whoami(ext, Parameters(serde_json::from_value(args).unwrap()))
                .await
        }
        other => panic!("no harness arm for {other}"),
    };
    match r {
        Ok(mut res) => {
            if c.host_alias.is_some() {
                fx.t.redact_work_for(&c, &mut res);
            }
            if res.is_error == Some(true) {
                return Err(res
                    .content
                    .iter()
                    .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                    .collect());
            }
            Ok(res
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect())
        }
        Err(e) => Err(e.message.to_string()),
    }
}

/// The session a caller acts on as "its own": a host's own session, the
/// master's pick otherwise.
fn own(fx: &Fx, who: Who) -> i64 {
    match who {
        Who::HostB => fx.s_b,
        Who::HostNone => fx.s_n,
        _ => fx.s_a,
    }
}

/// Run one matrix row for every caller: the leak check, then `expect`.
struct Matrix<'a> {
    fx: &'a Fx,
    isolate: bool,
    covered: BTreeSet<(String, String)>,
}

impl Matrix<'_> {
    async fn row(
        &mut self,
        tool: &str,
        action: &str,
        args: impl Fn(&Fx, Who) -> Value,
        expect: impl Fn(&Fx, Who, &Answer),
    ) {
        self.covered.insert((tool.to_string(), action.to_string()));
        for &who in EVERYONE {
            let asked = args(self.fx, who);
            let asked_text = asked.to_string();
            let a = call(self.fx, who, tool, asked).await;
            // A marker the caller typed itself (a key it asked about) may
            // come back in the refusal; anything else it did not type is a
            // leak.
            for m in who
                .forbidden_markers()
                .into_iter()
                .filter(|m| !asked_text.contains(m))
            {
                assert!(
                    !text(&a).contains(m),
                    "LEAK: {who:?} read {m:?} through {tool} {action} \
                     (isolate_sessions={}): {}",
                    self.isolate,
                    text(&a)
                );
            }
            expect(self.fx, who, &a);
        }
    }
}

fn is_ok(who: Who, a: &Answer, ctx: &str) {
    assert!(a.is_ok(), "{who:?} {ctx}: {a:?}");
}

fn is_code(who: Who, a: &Answer, want: &str, ctx: &str) {
    assert_eq!(code(a), want, "{who:?} {ctx}: {a:?}");
}

/// A mutating tool: the readonly client is refused by its mode.
fn readonly_refused(who: Who, a: &Answer) -> bool {
    if who == Who::ClientReadonly {
        assert_eq!(code(a), "E_FORBIDDEN", "readonly: {a:?}");
        return true;
    }
    false
}

/// Same code and same sentence with the id / key swapped: no oracle.
fn same_as_unknown(a: &Answer, unknown: &Answer, id: &str, unknown_id: &str) {
    assert_eq!(code(a), code(unknown), "{a:?} vs {unknown:?}");
    // The id named is the last one in the sentence; ids before it (the
    // caller's own session) are the caller's own.
    let swap_last = |t: &str, x: &str| match t.rfind(x) {
        Some(i) => format!("{}<X>{}", &t[..i], &t[i + x.len()..]),
        None => t.to_string(),
    };
    assert_eq!(
        swap_last(text(a), id),
        swap_last(text(unknown), unknown_id),
        "an out-of-scope id must read exactly as an unknown one"
    );
}

async fn run_matrix(isolate: bool) {
    let fx = fixture(isolate);
    let mut m = Matrix {
        fx: &fx,
        isolate,
        covered: BTreeSet::new(),
    };

    // ── work (reads) ────────────────────────────────────────────────────
    m.row(
        "work",
        "links",
        |fx, who| json!({ "action": "links", "session_id": own(fx, who) }),
        |_, who, a| is_ok(who, a, "own session's links"),
    )
    .await;
    // Another host's session: the host fence answers first.
    m.row(
        "work",
        "links",
        |fx, _| json!({ "session_id": fx.s_b }),
        |_, who, a| match who {
            Who::HostA | Who::HostNone => is_code(who, a, "E_FORBIDDEN", "other host"),
            _ => is_ok(who, a, "links of s_b"),
        },
    )
    .await;
    // s_x (h-a) carries B's ticket: host A reads the session's links
    // without it; the master sees it.
    m.row(
        "work",
        "links",
        |fx, _| json!({ "session_id": fx.s_x }),
        |fx, who, a| match who {
            Who::HostA => {
                is_ok(who, a, "own host");
                assert_eq!(text(a), "[]", "the forced B link is not host A's to read");
            }
            Who::Master | Who::ClientFull | Who::ClientReadonly => assert!(
                text(a).contains(&format!("\"item_id\":{}", fx.item_b)),
                "{who:?}: {a:?}"
            ),
            _ => is_code(who, a, "E_FORBIDDEN", "other host"),
        },
    )
    .await;
    m.row(
        "work",
        "links",
        |_, _| json!({ "key": "BB-3" }),
        |_, who, a| match who {
            Who::HostB => assert!(text(a).contains("\"snap_host\":\"h-b\""), "{a:?}"),
            w if w.is_host() => assert_eq!(text(a), "[]", "{who:?}"),
            _ => assert!(text(a).contains("h-b"), "{who:?}: {a:?}"),
        },
    )
    .await;
    m.row(
        "work",
        "links",
        |_, _| json!({}),
        |_, who, a| is_ok(who, a, "recent past work"),
    )
    .await;
    for key in ["BB-1", "BB-3"] {
        m.row(
            "work",
            "context",
            move |_, _| json!({ "action": "context", "key": key }),
            move |_, who, a| match who {
                Who::HostA | Who::HostNone => {
                    is_code(who, a, "E_FORBIDDEN", "another org's context")
                }
                Who::HostB => assert!(text(a).contains("SECRET-B"), "{who:?} {key}: {a:?}"),
                _ => assert!(text(a).contains("SECRET-B"), "{who:?} {key}: {a:?}"),
            },
        )
        .await;
    }
    // No oracle: an unknown key answers host A exactly as B's key does.
    let hidden = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "context", "key": "BB-1" }),
    )
    .await;
    let unknown = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "context", "key": "ZZ-404" }),
    )
    .await;
    same_as_unknown(&hidden, &unknown, "BB-1", "ZZ-404");
    // Host A's own work: its context carries A and nothing of B.
    let own_ctx = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "context", "key": "AA-1" }),
    )
    .await;
    assert!(text(&own_ctx).contains("SECRET-A"), "{own_ctx:?}");
    m.row(
        "work",
        "resume_plan",
        |_, _| json!({ "action": "resume_plan", "key": "BB-3" }),
        |_, who, a| match who {
            Who::HostA | Who::HostNone => {
                is_code(who, a, "E_FORBIDDEN", "another org's resume plan")
            }
            _ => is_ok(who, a, "resume plan"),
        },
    )
    .await;
    // A host cannot borrow another host's scope for a brief: it names h-b
    // as the landing host for work both orgs touched (a bare key).
    {
        let s = fx.t.store.lock().unwrap();
        for sid in [fx.s_a, fx.s_b] {
            s.link_session_work(sid, WorkTarget::Key("FOO-7"), "manual")
                .unwrap();
        }
    }
    let borrowed = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "resume_plan", "key": "FOO-7", "host_alias": "h-b", "with_brief": true }),
    )
    .await;
    assert!(!text(&borrowed).contains("SECRET-B"), "{borrowed:?}");
    let ctx = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "context", "key": "FOO-7" }),
    )
    .await;
    assert!(
        text(&ctx).contains("SECRET-A") && !text(&ctx).contains("SECRET-B"),
        "{ctx:?}"
    );
    {
        let s = fx.t.store.lock().unwrap();
        s.conn_for_test()
            .execute("DELETE FROM work_links WHERE ref_key = 'FOO-7'", [])
            .unwrap();
        for sid in [fx.s_a, fx.s_b] {
            crate::service::work::detect::resolve_session(&s, sid).unwrap();
        }
    }
    m.row(
        "work",
        "purge_impact",
        |fx, who| {
            let host = match who {
                Who::HostA => "h-a",
                Who::HostNone => "h-n",
                _ => "h-b",
            };
            json!({ "action": "purge_impact", "project_id": fx.pid_beta,
                    "host_aliases": [host] })
        },
        |_, who, a| is_ok(who, a, "purge impact"),
    )
    .await;
    m.row(
        "work",
        "tickets",
        |_, _| json!({ "action": "tickets" }),
        |_, who, a| {
            let keys: Vec<String> = serde_json::from_str::<Vec<Value>>(text(a))
                .unwrap()
                .iter()
                .filter_map(|t| t["key"].as_str().map(String::from))
                .collect();
            match who {
                Who::HostA => assert_eq!(keys, vec!["AA-1"], "own org, own host"),
                Who::HostB => {
                    let mut k = keys.clone();
                    k.sort();
                    // BB-3's past session ran on h-b too.
                    assert_eq!(k, vec!["BB-1", "BB-3"], "own org, own host")
                }
                Who::HostNone => assert!(keys.is_empty(), "a host in no org: {keys:?}"),
                _ => assert_eq!(keys.len(), 4, "{who:?}: {keys:?}"),
            }
        },
    )
    .await;
    // Tracker id guessing: B's tracker answers host A as an unknown one.
    m.row(
        "work",
        "tickets",
        |fx, _| json!({ "action": "tickets", "tracker_id": fx.tracker_b }),
        |_, who, a| {
            if matches!(who, Who::HostA | Who::HostNone) {
                assert_eq!(text(a), "[]", "{who:?}");
            }
        },
    )
    .await;
    let guessed = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "tickets", "tracker_id": fx.tracker_b }),
    )
    .await;
    let nothing = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "tickets", "tracker_id": 999_999 }),
    )
    .await;
    assert_eq!(
        guessed, nothing,
        "tracker_id of another org reads as unknown"
    );
    for (key, url) in [
        ("BB-1", "https://bravo.atlassian.net/browse/BB-1"),
        ("AA-1", "https://alpha.atlassian.net/browse/AA-1"),
    ] {
        for args in [
            json!({ "action": "lookup", "key": key }),
            json!({ "action": "lookup", "url": url }),
        ] {
            m.row(
                "work",
                "lookup",
                |_, _| args.clone(),
                |_, who, a| {
                    let mine = matches!((who, key), (Who::HostA, "AA-1") | (Who::HostB, "BB-1"));
                    if who.is_host() && !mine {
                        is_code(who, a, "E_FORBIDDEN", "another org's ticket")
                    } else {
                        is_ok(who, a, "lookup");
                        assert!(text(a).contains(key), "{who:?}: {a:?}");
                    }
                },
            )
            .await;
        }
    }
    let hidden = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "lookup", "key": "BB-1" }),
    )
    .await;
    let unknown = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "lookup", "key": "ZZ-404" }),
    )
    .await;
    same_as_unknown(&hidden, &unknown, "BB-1", "ZZ-404");
    m.row(
        "work",
        "trackers",
        |_, _| json!({ "action": "trackers" }),
        |fx, who, a| {
            let ids: Vec<i64> = serde_json::from_str::<Vec<Value>>(text(a))
                .unwrap()
                .iter()
                .filter_map(|t| t["id"].as_i64())
                .collect();
            match who {
                Who::HostA => assert_eq!(ids, vec![fx.tracker_a]),
                Who::HostB => assert_eq!(ids, vec![fx.tracker_b]),
                Who::HostNone => assert!(ids.is_empty()),
                _ => assert_eq!(ids.len(), 2),
            }
        },
    )
    .await;
    // The ticket card (work graph M9.2): from the cache, and to a host only
    // for its own work — with the tracker text fenced, never plain.
    for key in ["BB-1", "AA-1"] {
        m.row(
            "work",
            "card",
            move |_, _| json!({ "action": "card", "key": key }),
            move |_, who, a| {
                let mine = matches!((who, key), (Who::HostA, "AA-1") | (Who::HostB, "BB-1"));
                if who.is_host() && !mine {
                    return is_code(who, a, "E_FORBIDDEN", "another org's card");
                }
                is_ok(who, a, "card");
                let v: Value = serde_json::from_str(text(a)).unwrap();
                let secret = if key == "BB-1" {
                    "SECRET-B"
                } else {
                    "SECRET-A"
                };
                let fenced = v["composer_text"].as_str().unwrap();
                assert!(
                    fenced.contains(secret) && fenced.contains(crate::mcp::guard::UNTRUSTED_END),
                    "{who:?}: {v}"
                );
                if who.is_host() {
                    assert!(
                        v.get("excerpt").is_none() && v.get("acceptance").is_none(),
                        "{v}"
                    );
                } else {
                    assert!(v["excerpt"].as_str().unwrap().contains(secret), "{v}");
                }
            },
        )
        .await;
    }
    let hidden = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "card", "key": "BB-1" }),
    )
    .await;
    let unknown = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "card", "key": "ZZ-404" }),
    )
    .await;
    same_as_unknown(&hidden, &unknown, "BB-1", "ZZ-404");
    // Multi-repo start (work graph M9.6): each repo is planned under the
    // caller's scope, so a host that cannot see B's ticket starts nothing.
    m.row(
        "work_link",
        "start",
        |_, _| json!({ "action": "start", "key": "BB-2", "project_ids": [1, 2], "host_alias": "h-a" }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            let v: Value = serde_json::from_str(text(a)).unwrap();
            assert_eq!(v["started"], json!([]), "{who:?}: {v}");
            if matches!(who, Who::HostA | Who::HostNone) {
                assert!(
                    v["failed"].as_array().unwrap().iter().all(|f| f["code"] == "E_FORBIDDEN"),
                    "{who:?}: {v}"
                );
            }
        },
    )
    .await;
    // Agent-written handover (work graph M9.3): a caller asks only its own
    // sessions; the send itself fails here for want of a real host.
    m.row(
        "work_link",
        "handover",
        |fx, who| json!({ "action": "handover", "session_id": own(fx, who) }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            assert!(
                !matches!(code(a), "E_FORBIDDEN" | "E_NOTFOUND" | "E_INVALID")
                    && !text(a).contains("not visible"),
                "{who:?}: {a:?}"
            );
        },
    )
    .await;
    // Host A on s_x (A's session carrying B's ticket): the work is not A's
    // to read, so the session reads as having none.
    let x = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "handover", "session_id": fx.s_x }),
    )
    .await;
    is_code(Who::HostA, &x, "E_INVALID", "B's work on an A session");
    let hidden = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "handover", "session_id": fx.s_b }),
    )
    .await;
    let unknown = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "handover", "session_id": 999_999 }),
    )
    .await;
    same_as_unknown(&hidden, &unknown, &fx.s_b.to_string(), "999999");
    // Today (work graph M9.1): BB-3 shipped today (done, and its ended
    // session left a PR). Each host reads its own host's day inside its org.
    {
        let s = fx.t.store.lock().unwrap();
        let now = crate::service::catalog::now_secs();
        s.conn_for_test()
            .execute(
                "UPDATE work_items SET status_category = 'done', status_changed_at = ?1 \
                 WHERE id IN (SELECT item_id FROM work_links WHERE snap_tmux = 's-b-old')",
                [now],
            )
            .unwrap();
        s.conn_for_test()
            .execute(
                "UPDATE work_links SET snap_pr_url = 'https://github.com/beta/web/pull/3' \
                 WHERE snap_tmux = 's-b-old'",
                [],
            )
            .unwrap();
    }
    m.row(
        "work",
        "today",
        |_, _| json!({ "action": "today", "since": 0 }),
        |fx, who, a| {
            let v: Value = serde_json::from_str(text(a)).unwrap();
            let mut ids: Vec<i64> = v["groups"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|g| g["sessions"].as_array().unwrap().iter())
                .filter_map(|s| s["id"].as_i64())
                .collect();
            ids.sort();
            let shipped: Vec<&str> = v["shipped"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|s| s["key"].as_str())
                .collect();
            let mut want = match who {
                Who::HostA => vec![fx.s_a, fx.s_x],
                Who::HostB => vec![fx.s_b],
                Who::HostNone => vec![fx.s_n],
                _ => vec![fx.s_a, fx.s_b, fx.s_n, fx.s_x],
            };
            want.sort();
            assert_eq!(ids, want, "{who:?}: {v}");
            match who {
                Who::HostA | Who::HostNone => assert!(shipped.is_empty(), "{who:?}: {v}"),
                _ => assert_eq!(shipped, vec!["BB-3"], "{who:?}: {v}"),
            }
        },
    )
    .await;
    m.row(
        "work",
        "scopes",
        |_, _| json!({ "action": "scopes" }),
        |fx, who, a| {
            let v: Vec<Value> = serde_json::from_str(text(a)).unwrap();
            let named: Vec<i64> = v.iter().filter_map(|e| e["id"].as_i64()).collect();
            match who {
                Who::HostA => assert!(!named.contains(&fx.org_b), "{v:?}"),
                Who::HostNone => assert!(named.is_empty(), "{v:?}"),
                Who::HostB => assert_eq!(named, vec![fx.org_b]),
                _ => assert_eq!(named.len(), 2),
            }
        },
    )
    .await;
    m.row(
        "work",
        "orgs",
        |_, _| json!({ "action": "orgs" }),
        |fx, who, a| {
            let v: Vec<Value> = serde_json::from_str(text(a)).unwrap();
            match who {
                Who::HostA => assert!(v.iter().all(|o| o["id"] != fx.org_b)),
                Who::HostNone => assert!(v.is_empty()),
                _ => assert!(!v.is_empty()),
            }
        },
    )
    .await;
    m.row(
        "work",
        "org_suggestions",
        |_, _| json!({ "action": "org_suggestions" }),
        |_, who, a| {
            if who.is_host() {
                assert_eq!(text(a), "[]", "a host has nothing to act on");
            }
        },
    )
    .await;

    // ── work_link (writes) ──────────────────────────────────────────────
    // Another org's key on one's own session.
    m.row(
        "work_link",
        "link",
        |fx, who| {
            let key = if who == Who::HostB { "AA-1" } else { "BB-1" };
            json!({ "action": "link", "session_id": own(fx, who), "key": key })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            if who.is_host() {
                // A key outside the host's orgs links as the bare key it
                // typed — exactly what an unknown key does: no oracle.
                is_ok(who, a, "bare link");
                let row: Value = serde_json::from_str(text(a)).unwrap();
                assert!(row["work"]["item_id"].is_null(), "{who:?}: {a:?}");
            } else {
                // Master and clients: the cross-org integrity rule (s_a is
                // in A, BB-1 in B).
                is_code(who, a, "E_FORBIDDEN", "cross-org link");
                assert!(text(a).contains("force_cross_org"), "{a:?}");
            }
        },
    )
    .await;
    // The bare links the hosts made never bind to the other org's items,
    // and never make the other org's tracker fetch them.
    {
        let s = fx.t.store.lock().unwrap();
        for (tracker, key) in [(fx.tracker_b, "BB-1"), (fx.tracker_a, "AA-1")] {
            s.bind_tracker_refs(tracker).unwrap();
            let bare: i64 = s
                .conn_for_test()
                .query_row(
                    "SELECT COUNT(*) FROM work_links WHERE ref_key = ?1 AND item_id IS NULL",
                    [key],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(bare >= 1, "{key}: the host's bare link stays bare");
        }
        // Undo them, so later rows read the fixture as it was.
        s.conn_for_test()
            .execute(
                "DELETE FROM work_links WHERE item_id IS NULL AND ref_key IN ('BB-1', 'AA-1')",
                [],
            )
            .unwrap();
        for sid in [fx.s_a, fx.s_b, fx.s_n] {
            crate::service::work::detect::resolve_session(&s, sid).unwrap();
        }
    }
    // An unknown key and another org's key answer a host the same way.
    let other = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "reject", "session_id": fx.s_a, "key": "BB-2" }),
    )
    .await;
    let unknown = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "reject", "session_id": fx.s_a, "key": "ZZ-2" }),
    )
    .await;
    assert_eq!(code(&other), code(&unknown), "{other:?} vs {unknown:?}");
    {
        let s = fx.t.store.lock().unwrap();
        s.conn_for_test()
            .execute(
                "DELETE FROM work_links WHERE item_id IS NULL AND ref_key IN ('BB-2', 'ZZ-2')",
                [],
            )
            .unwrap();
    }
    // Item id guessing.
    m.row(
        "work_link",
        "link",
        |fx, who| {
            let item = if who == Who::HostB {
                fx.item_a
            } else {
                fx.item_b
            };
            json!({ "action": "link", "session_id": own(fx, who), "item_id": item })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            if who.is_host() {
                is_code(who, a, "E_NOTFOUND", "another org's item id");
            } else {
                is_code(who, a, "E_FORBIDDEN", "cross-org by item id");
            }
        },
    )
    .await;
    let guessed = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "link", "session_id": fx.s_a, "item_id": fx.item_b }),
    )
    .await;
    let unknown = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "link", "session_id": fx.s_a, "item_id": 999_999 }),
    )
    .await;
    same_as_unknown(&guessed, &unknown, &fx.item_b.to_string(), "999999");
    // The master forces it, then undoes it.
    let forced = call(
        &fx,
        Who::Master,
        "work_link",
        json!({ "action": "link", "session_id": fx.s_a, "key": "BB-2", "force_cross_org": true }),
    )
    .await;
    assert!(text(&forced).contains("BB-2"), "{forced:?}");
    let links = call(&fx, Who::Master, "work", json!({ "session_id": fx.s_a })).await;
    let forced_id = serde_json::from_str::<Vec<Value>>(text(&links))
        .unwrap()
        .into_iter()
        .find(|l| l["ref_key"] == "BB-2")
        .and_then(|l| l["id"].as_i64())
        .unwrap();
    // Host A cannot see — nor unlink — what the master forced on its session.
    let hidden = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "unlink", "session_id": fx.s_a, "link_id": forced_id }),
    )
    .await;
    let unknown = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "unlink", "session_id": fx.s_a, "link_id": 999_999 }),
    )
    .await;
    same_as_unknown(&hidden, &unknown, &forced_id.to_string(), "999999");
    is_ok(
        Who::Master,
        &call(
            &fx,
            Who::Master,
            "work_link",
            json!({ "action": "unlink", "session_id": fx.s_a, "link_id": forced_id }),
        )
        .await,
        "undo",
    );
    m.row(
        "work_link",
        "reject",
        |fx, who| {
            let item = if who == Who::HostB {
                fx.item_a
            } else {
                fx.item_b
            };
            json!({ "action": "reject", "session_id": own(fx, who), "item_id": item })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            if who.is_host() {
                is_code(who, a, "E_NOTFOUND", "reject another org's item");
            } else {
                is_ok(who, a, "a rejection is no link");
            }
        },
    )
    .await;
    // Link id guessing: s_x's forced link (host A's own session) and s_b's.
    for pick in [0, 1] {
        for action in ["confirm", "unlink"] {
            m.row(
                "work_link",
                action,
                move |fx, who| {
                    let (sid, link) = if pick == 0 {
                        (fx.s_x, fx.link_x)
                    } else {
                        (own(fx, who), fx.link_b)
                    };
                    let sid = if who.is_host() && pick == 0 && who != Who::HostA {
                        own(fx, who)
                    } else {
                        sid
                    };
                    json!({ "action": action, "session_id": sid, "link_id": link })
                },
                move |_, who, a| {
                    if readonly_refused(who, a) {
                        return;
                    }
                    if who.is_host() && !(who == Who::HostB && pick == 1) {
                        is_code(who, a, "E_NOTFOUND", "another org's link id");
                    }
                },
            )
            .await;
        }
    }
    let guessed = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "confirm", "session_id": fx.s_x, "link_id": fx.link_x }),
    )
    .await;
    let unknown = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "confirm", "session_id": fx.s_x, "link_id": 999_999 }),
    )
    .await;
    same_as_unknown(&guessed, &unknown, &fx.link_x.to_string(), "999999");
    m.row(
        "work_link",
        "trust_project",
        |fx, _| json!({ "action": "trust_project", "project_id": fx.pid_beta, "on": false }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            if who.is_host() {
                is_code(who, a, "E_FORBIDDEN", "trust is fleet configuration");
            } else {
                is_ok(who, a, "trust");
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "name",
        |_, _| json!({ "action": "name", "key": "LOC-1", "name": "Local work" }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            if who.is_host() {
                is_code(who, a, "E_FORBIDDEN", "local items are fleet-wide");
            } else {
                is_ok(who, a, "name");
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "resume",
        |_, _| json!({ "action": "resume", "key": "BB-3", "mode": "fresh" }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostA | Who::HostNone => is_code(who, a, "E_FORBIDDEN", "resume B's work"),
                // Everyone else gets as far as the spawn, which fails
                // here for want of a real host — never an org refusal.
                _ => assert!(!text(a).contains("not visible"), "{who:?}: {a:?}"),
            }
        },
    )
    .await;
    for args in [
        json!({ "action": "start", "key": "BB-2", "project_id": 1, "host_alias": "h-a" }),
        json!({ "action": "start", "url": "https://bravo.atlassian.net/browse/BB-2",
                "project_id": 1, "host_alias": "h-a" }),
    ] {
        m.row(
            "work_link",
            "start",
            |_, _| args.clone(),
            |_, who, a| {
                if readonly_refused(who, a) {
                    return;
                }
                // Hosts: not visible (host B: not linked on its host, and
                // h-a is not its host). Master and clients: BB-2 on an A
                // session is the integrity refusal.
                is_code(who, a, "E_FORBIDDEN", "start B's ticket on A");
            },
        )
        .await;
    }
    m.row(
        "work_link",
        "start",
        |fx, _| json!({ "action": "start", "item_id": fx.item_b, "project_id": 1 }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            if matches!(who, Who::HostA | Who::HostNone) {
                is_code(who, a, "E_NOTFOUND", "start by another org's item id");
            }
        },
    )
    .await;
    let guessed = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "start", "item_id": fx.item_b }),
    )
    .await;
    let unknown = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "start", "item_id": 999_999 }),
    )
    .await;
    same_as_unknown(&guessed, &unknown, &fx.item_b.to_string(), "999999");

    // ── work_admin: the master's, and nobody else's ─────────────────────
    for name in AdminAction::NAMES {
        let name = *name;
        m.row(
            "work_admin",
            name,
            move |_, _| json!({ "action": name }),
            move |_, who, a| match who {
                Who::Master => {
                    if matches!(name, "list" | "list_orgs") {
                        is_ok(who, a, name);
                    }
                }
                _ => is_code(who, a, "E_FORBIDDEN", "work_admin is master-only"),
            },
        )
        .await;
    }

    // ── sessions (D7) ───────────────────────────────────────────────────
    let isolated = isolate;
    m.row(
        "list_sessions",
        "-",
        |_, _| json!({ "summary": false }),
        move |fx, who, a| {
            let rows: Vec<Value> = serde_json::from_str(text(a)).unwrap();
            let sees_b = rows.iter().any(|r| r["id"] == fx.s_b);
            let sees_a = rows.iter().any(|r| r["id"] == fx.s_a);
            match who {
                Who::HostA | Who::HostNone => assert_eq!(sees_b, !isolated, "{who:?}"),
                Who::HostB => {
                    assert!(sees_b);
                    assert_eq!(
                        sees_a, !isolated,
                        "an isolating org's host sees only its own"
                    );
                }
                _ => assert!(sees_b && sees_a, "{who:?}"),
            }
            // Nobody but the master and clients reads B's work on a row.
            for r in &rows {
                if who == Who::HostA && r["id"] == fx.s_x {
                    assert!(r.get("work").is_none(), "{r}");
                }
            }
            if !who.is_host() {
                assert!(text(a).contains("BB-1"));
            }
        },
    )
    .await;
    m.row(
        "peer_status",
        "-",
        |fx, _| json!({ "session_id": fx.s_b }),
        move |_, who, a| match who {
            Who::HostA | Who::HostNone if isolated => {
                is_code(who, a, "E_NOTFOUND", "isolated peer")
            }
            _ => is_ok(who, a, "peer status"),
        },
    )
    .await;
    if isolate {
        let hidden = call(
            &fx,
            Who::HostA,
            "peer_status",
            json!({ "session_id": fx.s_b }),
        )
        .await;
        let unknown = call(
            &fx,
            Who::HostA,
            "peer_status",
            json!({ "session_id": 999_999 }),
        )
        .await;
        same_as_unknown(&hidden, &unknown, &fx.s_b.to_string(), "999999");
    }
    m.row(
        "related_sessions",
        "-",
        |fx, _| json!({ "session_id": fx.s_b }),
        move |_, who, a| match who {
            // An isolated anchor answers exactly as a missing one.
            Who::HostA | Who::HostNone if isolated => {
                is_code(who, a, "E_SQLITE", "isolated anchor")
            }
            _ => is_ok(who, a, "related"),
        },
    )
    .await;
    if isolate {
        let hidden = call(
            &fx,
            Who::HostA,
            "related_sessions",
            json!({ "session_id": fx.s_b }),
        )
        .await;
        let missing = call(
            &fx,
            Who::HostA,
            "related_sessions",
            json!({ "session_id": 999_999 }),
        )
        .await;
        assert_eq!(hidden, missing);
    }
    m.row(
        "send_message",
        "-",
        |fx, who| {
            json!({ "from_session_id": own(fx, who), "to_session_id": fx.s_b,
                    "body": "hello", "deliver": false })
        },
        move |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostA | Who::HostNone if isolated => {
                    is_code(who, a, "E_NOTFOUND", "message to an isolated session")
                }
                Who::HostB => is_code(who, a, "E_SELF_TARGET", "own session"),
                _ => is_ok(who, a, "message"),
            }
        },
    )
    .await;
    // A controller in A dispatching to a worker in A still works: a message
    // between A's own sessions is never fenced.
    let within = call(&fx, Who::HostA, "send_message", json!({ "from_session_id": fx.s_a, "to_session_id": fx.s_x, "body": "task", "deliver": false })).await;
    is_ok(Who::HostA, &within, "within org A");

    // Session-addressed reads a host token may make of ANY host's session:
    // an isolated org's session reads as missing.
    for (tool, args) in [
        ("session_history", json!({ "session_id": fx.s_b })),
        ("repo_changes", json!({ "session_id": fx.s_b })),
        ("whoami", json!({ "tmux_name": "s-b" })),
    ] {
        for &who in EVERYONE {
            let a = call(&fx, who, tool, args.clone()).await;
            for mk in who.forbidden_markers() {
                assert!(!text(&a).contains(mk), "LEAK {who:?} {tool}: {a:?}");
            }
            if isolate && matches!(who, Who::HostA | Who::HostNone) {
                is_code(who, &a, "E_NOTFOUND", tool);
            }
        }
    }

    // ── /events: the per-frame fence ────────────────────────────────────
    let rows: Vec<crate::store::SessionRow> = {
        let s = fx.t.store.lock().unwrap();
        [fx.s_a, fx.s_b, fx.s_n, fx.s_x]
            .iter()
            .map(|id| s.get_session_by_id(*id).unwrap().unwrap())
            .collect()
    };
    for &who in EVERYONE {
        let c = who.caller();
        let scope = {
            let s = fx.t.store.lock().unwrap();
            c.org_scope(&s).unwrap()
        };
        let kinds = crate::mcp::events_route::fence_host_bound(&c, None);
        if who.is_host() {
            assert!(
                !kinds.unwrap().iter().any(|k| k == "work"),
                "work frames never reach a host"
            );
        }
        for (i, row) in rows.iter().enumerate() {
            let msg = crate::events::EventMessage {
                name: "session:updated",
                payload: serde_json::to_value(row).unwrap(),
                seq: i as u64 + 1,
            };
            let lookup = |sid: i64| {
                let s = fx.t.store.lock().unwrap();
                s.get_session_by_id(sid)
                    .unwrap()
                    .map(|r| (r.host_alias, r.org_id))
            };
            // The same session's timeline frame (no row, only its id).
            let ev = crate::events::EventMessage {
                name: "session:event",
                payload: json!({ "id": 1, "session_id": row.id, "kind": "prompt_sent" }),
                seq: 100 + i as u64,
            };
            let ev_out = crate::mcp::events_route::fence_frame(&scope, &ev, &lookup);
            let out = crate::mcp::events_route::fence_frame(&scope, &msg, &lookup);
            assert_eq!(
                ev_out.is_none(),
                out.is_none(),
                "{who:?}: a row and its events agree"
            );
            let is_b = row.id == fx.s_b;
            let is_a = row.id == fx.s_a || row.id == fx.s_x;
            // B isolates: nobody outside B reads B's session frames, and
            // B's hosts read only B's and unassigned ones.
            let dropped = isolate
                && ((matches!(who, Who::HostA | Who::HostNone) && is_b)
                    || (who == Who::HostB && is_a));
            if dropped {
                assert!(out.is_none(), "{who:?} got an isolated frame of {}", row.id);
                continue;
            }
            let out = out.expect("frame kept").to_string();
            for m in who.forbidden_markers() {
                assert!(
                    !out.contains(m),
                    "LEAK in a frame: {who:?} read {m:?}: {out}"
                );
            }
        }
    }

    // ── SessionStart context (M4.5) and handover briefs ────────────────
    {
        let store = Arc::clone(&fx.t.store);
        {
            let s = store.lock().unwrap();
            crate::service::settings::set(
                &s,
                crate::service::settings::WORK_SESSION_START_CONTEXT,
                "true",
            )
            .unwrap();
        }
        let start = |conv: &str, host: &str| {
            let p = crate::mcp::hooks::HookPayload {
                session_id: Some(conv.into()),
                hook_event_name: Some("SessionStart".into()),
                source: Some("startup".into()),
                ..Default::default()
            };
            let c = Caller {
                host_alias: Some(host.into()),
                client: None,
                mode: TokenMode::Full,
            };
            let ctx = crate::service::hooks::HookContext {
                caller: &c,
                pane_id: None,
            };
            crate::service::hooks::session_start_context(&store, &p, &ctx)
        };
        // s_x on h-a carries B's ticket: its Claude is told nothing of it.
        let x = start("conv-x", "h-a");
        assert!(
            x.as_deref()
                .is_none_or(|t| B_MARKERS.iter().all(|m| !t.contains(m))),
            "{x:?}"
        );
        let a = start("conv-a", "h-a").expect("A's own context");
        assert!(a.contains("AA-1") && a.contains("Alpha"), "{a}");
    }
    // A resume brief is written for the landing host: B's past work landing
    // on h-a (the master's choice) carries none of B's text; on h-b it does.
    for (host, leaks) in [("h-a", false), ("h-b", true)] {
        let plan = call(&fx, Who::Master, "work", json!({ "action": "resume_plan", "key": "BB-3", "host_alias": host, "with_brief": true })).await;
        let brief: Value = serde_json::from_str(text(&plan)).unwrap();
        let brief = brief["brief"].as_str().unwrap_or_default().to_string();
        assert_eq!(brief.contains("SECRET-B"), leaks, "{host}: {brief}");
    }
    let _ = (fx.item_a, fx.s_n);

    // ── lifecycle (work graph M7, on M5) ───────────────────────────────
    // Make s_b (B's, on h-b) and s_x (A's session carrying B's ticket, on
    // h-a) tidy candidates: idle, their tickets done long ago. Earlier rows
    // changed their links, so they are linked again here.
    let x_link = {
        let s = fx.t.store.lock().unwrap();
        s.link_session_work(fx.s_b, WorkTarget::Item(fx.item_b), "manual")
            .unwrap();
        let x_link = s
            .link_session_work(fx.s_x, WorkTarget::Item(fx.item_b), "manual")
            .unwrap()
            .id;
        s.conn_for_test()
            .execute_batch(&format!(
                "UPDATE sessions SET claude_status = 'idle', idle_since = 0 \
                   WHERE id IN ({}, {}); \
                 UPDATE work_items SET status_category = 'done', status_changed_at = 0 \
                   WHERE id = {}; \
                 UPDATE hosts SET reachable = 1;",
                fx.s_b, fx.s_x, fx.item_b
            ))
            .unwrap();
        x_link
    };
    m.row(
        "work",
        "tidy",
        |_, _| json!({ "action": "tidy" }),
        |fx, who, a| {
            is_ok(who, a, "tidy");
            let t = text(a);
            let has = |id: i64| t.contains(&format!("\"session_id\":{id}"));
            match who {
                // Its own host's and org's candidates only; B's ticket on its
                // own session is not named (the leak check covers the text).
                Who::HostA => assert!(has(fx.s_x) && !has(fx.s_b), "{t}"),
                Who::HostB => assert!(has(fx.s_b) && !has(fx.s_x), "{t}"),
                Who::HostNone => assert!(!has(fx.s_b) && !has(fx.s_x), "{t}"),
                _ => assert!(has(fx.s_b) && has(fx.s_x), "{who:?}: {t}"),
            }
        },
    )
    .await;
    m.row(
        "work",
        "reopened",
        |_, _| json!({ "action": "reopened" }),
        |_, who, a| is_ok(who, a, "reopened"),
    )
    .await;
    // Another host's session through tidy_apply: an unknown one, per item.
    m.row(
        "work_link",
        "tidy_apply",
        |fx, _| {
            json!({ "action": "tidy_apply",
                        "items": [{ "session_id": fx.s_b, "action": "snooze" }] })
        },
        |fx, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            is_ok(who, a, "a batch always answers");
            let refused = text(a).contains(&format!("session {} not found", fx.s_b));
            assert_eq!(
                refused,
                matches!(who, Who::HostA | Who::HostNone),
                "{who:?}: {a:?}"
            );
        },
    )
    .await;
    for action in ["archive", "unarchive", "never"] {
        m.row(
            "work_link",
            action,
            |fx, who| json!({ "action": action, "session_id": own(fx, who) }),
            |_, who, a| {
                if readonly_refused(who, a) {
                    return;
                }
                is_ok(who, a, "own session");
            },
        )
        .await;
    }
    // B's link on host A's own session cannot be snoozed by host A: it
    // reads as a link that does not exist.
    m.row(
        "work_link",
        "snooze",
        move |fx, _| json!({ "action": "snooze", "session_id": fx.s_x, "link_id": x_link }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostA => is_code(who, a, "E_NOTFOUND", "another org's link"),
                Who::HostB | Who::HostNone => is_code(who, a, "E_FORBIDDEN", "other host"),
                _ => is_ok(who, a, "snooze"),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "dismiss",
        |fx, _| json!({ "action": "dismiss", "item_id": fx.item_b }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            if who.is_host() {
                is_code(who, a, "E_FORBIDDEN", "fleet-wide");
            } else {
                is_ok(who, a, "dismiss");
            }
        },
    )
    .await;
    {
        let s = fx.t.store.lock().unwrap();
        s.conn_for_test()
            .execute_batch(&format!(
                "UPDATE sessions SET claude_status = NULL, idle_since = NULL \
                   WHERE id IN ({}, {}); \
                 UPDATE work_links SET archived_at = NULL, tidy_snoozed_until = NULL, \
                   tidy_never = 0;",
                fx.s_b, fx.s_x
            ))
            .unwrap();
    }

    // Coverage: every action of the three tools has a row.
    let want: BTreeSet<(String, String)> = WORK_ACTIONS
        .iter()
        .map(|(n, _)| ("work".to_string(), n.to_string()))
        .chain(
            WORK_LINK_ACTIONS
                .iter()
                .map(|n| ("work_link".to_string(), n.to_string())),
        )
        .chain(
            AdminAction::NAMES
                .iter()
                .map(|n| ("work_admin".to_string(), n.to_string())),
        )
        .collect();
    let missing: Vec<_> = want.difference(&m.covered).collect();
    assert!(
        missing.is_empty(),
        "actions without an isolation-matrix row: {missing:?} — add a row"
    );
}

#[tokio::test]
async fn the_isolation_matrix_holds_with_sessions_shared() {
    run_matrix(false).await;
}

#[tokio::test]
async fn the_isolation_matrix_holds_with_org_b_isolating_sessions() {
    run_matrix(true).await;
}

/// Every Routed work command of the desktop maps to an action the matrix
/// covers (src-tauri's routing tests hold `ROUTED_WORK_COMMANDS` to its
/// verdict table).
#[test]
fn every_routed_work_command_names_a_covered_action() {
    for (cmd, tool, action) in crate::service::work::ROUTED_WORK_COMMANDS {
        let known = match *tool {
            "work" => WORK_ACTIONS.iter().any(|(n, _)| n == action),
            "work_link" => WORK_LINK_ACTIONS.contains(action),
            _ => false,
        };
        assert!(
            known,
            "{cmd} → {tool} {action} is not an action the matrix runs"
        );
    }
}

/// M3's interim fence survives M5: two hosts of the SAME org still read
/// only the tickets their own sessions work on.
#[tokio::test]
async fn a_per_host_token_still_cannot_read_another_hosts_tickets_in_its_org() {
    let fx = fixture(false);
    {
        let s = fx.t.store.lock().unwrap();
        s.upsert_host("h-a2").unwrap();
        s.set_host_org("h-a2", s.host_org("h-a").unwrap()).unwrap();
    }
    let a2 = Caller {
        host_alias: Some("h-a2".into()),
        client: None,
        mode: TokenMode::Full,
    };
    let r =
        fx.t.work(
            Extension(a2.clone()),
            Parameters(serde_json::from_value(json!({ "action": "tickets" })).unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(
        r.content[0].as_text().unwrap().text,
        "[]",
        "AA-1 is h-a's, not h-a2's"
    );
    let e =
        fx.t.work(
            Extension(a2),
            Parameters(
                serde_json::from_value(json!({ "action": "lookup", "key": "AA-1" })).unwrap(),
            ),
        )
        .await
        .unwrap_err();
    assert!(e.message.starts_with("E_FORBIDDEN"), "{}", e.message);
}

/// A paired client can never reach a Master tool, whatever its mode, and a
/// host token never reaches org admin.
#[test]
fn nobody_but_the_master_administers_orgs() {
    for who in [
        Who::ClientFull,
        Who::ClientReadonly,
        Who::HostA,
        Who::HostNone,
    ] {
        assert!(
            enforce_admin(&who.caller(), "work_admin").is_err(),
            "{who:?}"
        );
    }
    assert!(enforce_admin(&Caller::master(), "work_admin").is_ok());
    assert!(!present::visible_to(
        &Who::ClientFull.caller(),
        "work_admin"
    ));
    assert!(!present::visible_to(&Who::HostA.caller(), "work_admin"));
    let _ = OrgScope::All;
}
