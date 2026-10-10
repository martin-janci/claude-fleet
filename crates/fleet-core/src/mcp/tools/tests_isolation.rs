//! The org isolation matrix (work graph M5.3) — the acceptance gate of the
//! boundary.
//!
//! Every `work` / `work_link` / `work_admin` action — enumerated from
//! `WORK_ACTIONS`, `WORK_LINK_ACTIONS` and `AdminAction::NAMES`, so a new
//! action without a row here fails `every_action_has_a_matrix_row` — runs
//! against eight callers (master, client full, client readonly, a host in
//! org A, a host in org B, a host in no org, and — work graph M14.1b — a
//! full client bound to org A and one bound to org B), with
//! `isolate_sessions` off and on for org B. The bound clients' second D31
//! value (`bound_sees_unassigned` off) is `the_work_view_reads_follow_d31_off`. Each call goes through what `call_tool` does around a tool:
//! the mode and admin gates before it, the org redaction after it.
//!
//! Two kinds of check run on every row:
//!
//! * **No leak.** Whatever a host-bound or org-bound caller gets back — a result or an
//!   error sentence — never contains another org's markers (title, key,
//!   journal and description text). A host in no org sees neither org's.
//! * **The row's own expectation**: who gets what, which error, and that an
//!   id of another org answers exactly as an id that does not exist.

use super::*;
use crate::service::orgs::OrgScope;
use crate::service::trackers::admin::AdminAction;
use crate::service::work::{WORK_ACTIONS, WORK_LINK_ACTIONS};
use crate::store::{TrackerConfig, TrackerItemWrite, WorkTarget};
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
    /// A full paired client bound to org A / org B (work graph M14.1b).
    BoundA,
    BoundB,
}

const EVERYONE: &[Who] = &[
    Who::Master,
    Who::ClientFull,
    Who::ClientReadonly,
    Who::HostA,
    Who::HostB,
    Who::HostNone,
    Who::BoundA,
    Who::BoundB,
];

/// The fixture's org ids (the first two orgs it adds).
const ORG_A: i64 = 1;
const ORG_B: i64 = 2;

/// The hub's personal owner, minted by migration 098 into an empty store —
/// so it is row 1, exactly as the orgs above are 1 and 2. The fixture
/// asserts it rather than trusting it.
const PERSON: i64 = 1;

/// The pane each host's token is standing in (multi-user M1, §4.4). The
/// fixture writes these into `sessions.tmux_pane_id`, so the scope builder
/// resolves each host caller to exactly the one row its agent is in — the
/// same thing a provisioned host's `X-Fleet-Pane` header does in
/// production.
const PANE_A: &str = "%11";
const PANE_B: &str = "%12";
const PANE_N: &str = "%13";

impl Who {
    fn caller(self) -> Caller {
        let host = |h: &str, pane: &str| Caller {
            api: None,
            host_alias: Some(h.into()),
            client: None,
            mode: TokenMode::Full,
            pane: Some(pane.into()),
            is_personal_owner: false,
        };
        // Every paired client here is the hub's one person's device.
        // A device bound to NOBODY is a refusing scope (M1, T2's default
        // makes it unmintable), so a fixture that left `person_id` at
        // `None` would be testing the fail-closed path in every row rather
        // than the org boundary this matrix is about.
        let client = |mode, org_id| Caller {
            api: None,
            host_alias: None,
            client: Some(crate::mcp::auth::ClientRef {
                id: 7,
                name: "phone".into(),
                trusted: false,
                org_id,
                person_id: Some(PERSON),
            }),
            mode,
            pane: None,
            is_personal_owner: org_id.is_none(),
        };
        match self {
            Who::Master => Caller::master(),
            Who::ClientFull => client(TokenMode::Full, None),
            Who::ClientReadonly => client(TokenMode::Readonly, None),
            Who::BoundA => client(TokenMode::Full, Some(ORG_A)),
            Who::BoundB => client(TokenMode::Full, Some(ORG_B)),
            Who::HostA => host("h-a", PANE_A),
            Who::HostB => host("h-b", PANE_B),
            Who::HostNone => host("h-n", PANE_N),
        }
    }

    fn is_host(self) -> bool {
        matches!(self, Who::HostA | Who::HostB | Who::HostNone)
    }

    /// A client bound to an org (work graph M14).
    fn is_bound(self) -> bool {
        matches!(self, Who::BoundA | Who::BoundB)
    }

    /// Reads every org: the master and an unbound client.
    fn is_unbound(self) -> bool {
        matches!(self, Who::Master | Who::ClientFull | Who::ClientReadonly)
    }

    /// Markers this caller must never read.
    fn forbidden_markers(self) -> Vec<&'static str> {
        match self {
            Who::HostA | Who::BoundA => B_MARKERS.to_vec(),
            Who::HostB | Who::BoundB => A_MARKERS.to_vec(),
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
    pid_acme: i64,
    pid_beta: i64,
    /// s_x's forced link to BB-1.
    link_x: i64,
}

/// The whole scope for the two `structure` reads that take one (multi-user M1,
/// T8d): every case in this file is about the ORG half, so the person half is
/// the hub's own unrestricted reader with the org under test put back on it —
/// the same `vs` helper `service::work::view_tests` uses, for the same reason.
fn iso_view(scope: &OrgScope) -> crate::service::view_scope::ViewScope {
    crate::service::view_scope::ViewScope::internal().with_org(scope.clone())
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
    // No `local` host. The `list_sessions` row below reaches
    // `service::sessions::list_sessions`, which runs a real reconcile pass
    // when the last one is stale — and whether it does is decided by a
    // PROCESS-GLOBAL gate (`reconcile_gate()`), so it depends on what else the
    // suite is doing at that moment. With a `local` host that pass runs
    // `tmux list-sessions` on the machine the test is running on and adopts
    // whatever it finds, so the matrix's expected page became "the four
    // fixture rows, plus however many tmux sessions this developer happens to
    // have open". Off, as on a `fleet-hub serve`, the pass has only the three
    // unreachable fixture hosts to probe and finds nothing.
    s.set_setting(crate::service::hub::SETTING_LOCAL_HOST, "false")
        .unwrap();
    for h in ["h-a", "h-b", "h-n"] {
        s.upsert_host(h).unwrap();
    }
    s.conn_for_test()
        .execute("UPDATE hosts SET reachable = 1", [])
        .unwrap();
    let a = s.add_org("Company A", Some("#f00"), false).unwrap();
    let b = s.add_org("Company B", Some("#00f"), isolate_b).unwrap();
    assert_eq!(
        (a.id, b.id),
        (ORG_A, ORG_B),
        "the bound callers name these ids"
    );
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
    // Each tracker claims its org's keys: without a prefix no bare key
    // would ever bind, and the bind fence below would have nothing to
    // refuse.
    for (t, prefix) in [(ta.id, "AA"), (tb.id, "BB")] {
        s.set_tracker_probe(
            t,
            None,
            &TrackerConfig {
                key_prefixes: vec![prefix.into()],
                ..Default::default()
            },
        )
        .unwrap();
    }
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
    // Task 5: a warm describe cache for each org's own ticket, so the
    // `describe` isolation row needs no fake tracker transport — it only
    // proves the fence, not the fetch (already covered in
    // `service::work::describe`'s own tests).
    s.put_description(item_a, "SECRET-A full description", 26)
        .unwrap();
    s.put_description(item_b, "SECRET-B full description", 26)
        .unwrap();

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
    // Multi-user M1. Three of the four rows are OWNED by the hub's one
    // person and therefore `private`; s_x is deliberately left
    // `unclaimed`, which is what a reconcile-discovered row looks like and
    // is the other half of §4.4's host rule. Each owned row carries the
    // pane its host's token speaks from, so a host caller resolves to
    // exactly the session its agent is in — the production shape, where
    // reconcile writes `tmux_pane_id` and the MCP connection carries
    // `X-Fleet-Pane`.
    assert_eq!(
        s.personal_owner_id().unwrap(),
        Some(PERSON),
        "migration 098 mints the owner as row 1"
    );
    for (id, pane) in [(s_a, PANE_A), (s_b, PANE_B), (s_n, PANE_N)] {
        s.claim_if_unclaimed(id, Some(PERSON)).unwrap();
        s.conn_for_test()
            .execute(
                "UPDATE sessions SET tmux_pane_id = ?2 WHERE id = ?1",
                rusqlite::params![id, pane],
            )
            .unwrap();
    }
    s.link_session_work(s_a, WorkTarget::Item(item_a), "manual")
        .unwrap();
    s.link_session_work(s_b, WorkTarget::Item(item_b), "manual")
        .unwrap();
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
        pid_acme,
        pid_beta,
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
        "fleet_health" => fx.t.fleet_health(ext).await,
        "usage_report" => {
            fx.t.usage_report(ext, Parameters(serde_json::from_value(args).unwrap()))
                .await
        }
        other => panic!("no harness arm for {other}"),
    };
    match r {
        Ok(mut res) => {
            if c.is_scoped() {
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
        Who::HostB | Who::BoundB => fx.s_b,
        Who::HostNone => fx.s_n,
        _ => fx.s_a,
    }
}

/// `session_id`'s live link to `item`, made through the store (which takes
/// a cross-org link, as the fixture's forced link was) when an earlier row
/// removed it; the id of the live link either way.
fn relink(fx: &Fx, session_id: i64, item: i64) -> i64 {
    fx.t.store
        .lock()
        .unwrap()
        .link_session_work(session_id, WorkTarget::Item(item), "manual")
        .unwrap()
        .id
}

fn link_is_live(fx: &Fx, link_id: i64) -> bool {
    fx.t.store
        .lock()
        .unwrap()
        .get_work_link(link_id)
        .unwrap()
        .is_some_and(|l| l.ended_at.is_none())
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
            // A client bound to A never reaches B's session (M14).
            Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's session"),
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
            // s_x is an A session: A's bound client reads it without B's
            // ticket; B's never reaches it (M14).
            Who::BoundA => {
                is_ok(who, a, "own org's session");
                assert_eq!(text(a), "[]", "the forced B link is not A's to read");
            }
            Who::BoundB => is_code(who, a, "E_NOTFOUND", "another org's session"),
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
            Who::BoundA => assert_eq!(text(a), "[]", "B's past work is not A's"),
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
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's context"),
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
            Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's resume plan"),
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
                // A bound client: its org's tickets, no host fence (M14).
                Who::BoundA => assert_eq!(keys, vec!["AA-1"], "own org"),
                Who::BoundB => {
                    let mut k = keys.clone();
                    k.sort();
                    assert_eq!(k, vec!["BB-1", "BB-2", "BB-3"], "own org")
                }
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
            if matches!(who, Who::HostA | Who::HostNone | Who::BoundA) {
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
                    let mine = matches!(
                        (who, key),
                        (Who::HostA | Who::BoundA, "AA-1") | (Who::HostB | Who::BoundB, "BB-1")
                    );
                    if who.is_host() && !mine {
                        is_code(who, a, "E_FORBIDDEN", "another org's ticket")
                    } else if who.is_bound() && !mine {
                        is_code(who, a, "E_NOTFOUND", "another org's ticket")
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
                Who::HostA | Who::BoundA => assert_eq!(ids, vec![fx.tracker_a]),
                Who::HostB | Who::BoundB => assert_eq!(ids, vec![fx.tracker_b]),
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
                let mine = matches!(
                    (who, key),
                    (Who::HostA | Who::BoundA, "AA-1") | (Who::HostB | Who::BoundB, "BB-1")
                );
                if who.is_host() && !mine {
                    return is_code(who, a, "E_FORBIDDEN", "another org's card");
                }
                if who.is_bound() && !mine {
                    return is_code(who, a, "E_NOTFOUND", "another org's card");
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
    // `describe` (Task 5 of the visible-truncation-and-describe plan):
    // exactly card's fence, served from the warm cache the fixture seeded
    // above — a per-host token reads only its own org's ticket, a bound
    // client the same by org, and everyone else sees both.
    for key in ["BB-1", "AA-1"] {
        m.row(
            "work",
            "describe",
            move |_, _| json!({ "action": "describe", "key": key }),
            move |_, who, a| {
                let mine = matches!(
                    (who, key),
                    (Who::HostA | Who::BoundA, "AA-1") | (Who::HostB | Who::BoundB, "BB-1")
                );
                if who.is_host() && !mine {
                    return is_code(who, a, "E_FORBIDDEN", "another org's ticket");
                }
                if who.is_bound() && !mine {
                    return is_code(who, a, "E_NOTFOUND", "another org's ticket");
                }
                is_ok(who, a, "describe");
                let secret = if key == "BB-1" {
                    "SECRET-B"
                } else {
                    "SECRET-A"
                };
                let v: Value = serde_json::from_str(text(a)).unwrap();
                assert_eq!(v["from_cache"], true, "{v}");
                assert!(v["body"].as_str().unwrap().contains(secret), "{who:?}: {v}");
            },
        )
        .await;
    }
    let hidden = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "describe", "key": "BB-1" }),
    )
    .await;
    let unknown = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "describe", "key": "ZZ-404" }),
    )
    .await;
    same_as_unknown(&hidden, &unknown, "BB-1", "ZZ-404");
    // Multi-repo start (work graph M9.6), per caller. The ticket is
    // resolved once under the caller's scope, then each repo is planned:
    // on h-a, acme/api is org A and beta/web org B (the owner rules).
    //
    // * master / full client: B's BB-2 is a cross-org refusal in A's repo
    //   (marked `cross_org`, so the UI offers "Start anyway") and a spawn
    //   attempt in B's — which fails here for want of a real host.
    // * host A: B's ticket is not its to read — refused outright; its own
    //   AA-1 is skipped where it already runs and cross-org in B's repo.
    // * host B: its own BB-1, but on h-a — the host fence, per repo, before
    //   the duplicate guard could name a session there.
    // * readonly: refused by its mode.
    m.row(
        "work_link",
        "start",
        |fx, who| {
            let key = if who == Who::HostB { "BB-1" } else { "BB-2" };
            json!({ "action": "start", "key": key,
                "project_ids": [fx.pid_acme, fx.pid_beta], "host_alias": "h-a" })
        },
        |fx, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            if matches!(who, Who::HostA | Who::HostNone) {
                return is_code(who, a, "E_FORBIDDEN", "another org's ticket");
            }
            is_ok(who, a, "multi-start");
            let v: Value = serde_json::from_str(text(a)).unwrap();
            let failed = v["failed"].as_array().cloned().unwrap_or_default();
            let of = |pid: i64| failed.iter().find(|f| f["project_id"] == pid).cloned();
            // A bound client (M14) never starts in the other org's repo:
            // the session would be invisible to it. BB-2 is not A's to read,
            // so for A it is a bare key (as an unknown key is).
            if who.is_bound() {
                let (other, own_pid) = if who == Who::BoundA {
                    (fx.pid_beta, fx.pid_acme)
                } else {
                    (fx.pid_acme, fx.pid_beta)
                };
                let f = of(other).unwrap_or_else(|| panic!("{who:?}: {v}"));
                assert_eq!(f["code"], "E_FORBIDDEN", "{who:?}: {v}");
                assert!(f["message"].as_str().unwrap().contains("bound"), "{v}");
                if let Some(f) = of(own_pid) {
                    assert_ne!(f["code"], "E_FORBIDDEN", "{who:?}: {v}");
                }
                return;
            }
            if who == Who::HostB {
                assert_eq!(v["key"], "BB-1", "{v}");
                assert_eq!(v["started"], json!([]), "{v}");
                assert!(v.get("skipped").is_none(), "the fence answers first: {v}");
                for pid in [fx.pid_acme, fx.pid_beta] {
                    let f = of(pid).unwrap_or_else(|| panic!("{pid}: {v}"));
                    assert_eq!(f["code"], "E_FORBIDDEN", "{v}");
                    assert!(f["message"].as_str().unwrap().contains("own host"), "{v}");
                }
                return;
            }
            assert_eq!(v["key"], "BB-2", "{v}");
            let acme = of(fx.pid_acme).unwrap_or_else(|| panic!("{v}"));
            assert_eq!(
                (&acme["code"], &acme["cross_org"]),
                (&json!("E_FORBIDDEN"), &json!(true)),
                "{who:?}: {v}"
            );
            // B's repo: planned, then a spawn attempt (no real host here).
            if let Some(f) = of(fx.pid_beta) {
                assert_ne!(f["code"], "E_FORBIDDEN", "{who:?}: {v}");
            }
        },
    )
    .await;
    // With force_cross_org the master's start reaches the spawn in both.
    let forced = call(
        &fx,
        Who::Master,
        "work_link",
        json!({ "action": "start", "key": "BB-2", "force_cross_org": true,
            "project_ids": [fx.pid_acme, fx.pid_beta], "host_alias": "h-a" }),
    )
    .await;
    let v: Value = serde_json::from_str(text(&forced)).unwrap();
    for f in v["failed"].as_array().cloned().unwrap_or_default() {
        assert_ne!(f["code"], "E_FORBIDDEN", "forced: {v}");
    }
    // Host A's own ticket: skipped where it runs (naming its session), a
    // cross-org refusal in B's repo.
    let own_a = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "start", "key": "AA-1",
            "project_ids": [fx.pid_acme, fx.pid_beta] }),
    )
    .await;
    let v: Value = serde_json::from_str(text(&own_a)).unwrap_or_else(|_| panic!("{own_a:?}"));
    assert_eq!(v["key"], "AA-1", "{v}");
    assert_eq!(v["skipped"][0]["project_id"], fx.pid_acme, "{v}");
    assert_eq!(v["skipped"][0]["session_id"], fx.s_a, "{v}");
    assert_eq!(v["failed"][0]["project_id"], fx.pid_beta, "{v}");
    assert_eq!(v["failed"][0]["cross_org"], true, "{v}");

    // The start's preview (task → session spec P-1) is fenced exactly as the
    // start: no way to read another org's ticket, plan on another host, or
    // name a session the caller may not see. A cross-org start is a
    // conflict to choose, never a refusal, for the master.
    let preview = |key: &str, pid: i64| {
        json!({ "action": "preview_start", "key": key,
            "project_id": pid, "host_alias": "h-a" })
    };
    m.row(
        "work_link",
        "preview_start",
        move |fx, who| {
            if who == Who::HostB {
                preview("BB-1", fx.pid_beta)
            } else {
                preview("BB-2", fx.pid_acme)
            }
        },
        |fx, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostA | Who::HostNone => {
                    is_code(who, a, "E_FORBIDDEN", "another org's ticket")
                }
                Who::HostB => is_code(who, a, "E_FORBIDDEN", "another host"),
                // B's ticket is not A's to read (a bare key to it), and an
                // A-repo session is A's: planned, with nothing to cross.
                Who::BoundA => {
                    is_ok(who, a, "own org's repo");
                    let v: Value = serde_json::from_str(text(a)).unwrap();
                    assert_eq!(v["plan"]["project_id"], fx.pid_acme, "{v}");
                    assert_eq!(v["title"], "", "no ticket text of another org: {v}");
                }
                // Never planned where the session would be another org's.
                Who::BoundB => is_code(who, a, "E_FORBIDDEN", "another org's repo"),
                _ => {
                    is_ok(who, a, "preview");
                    let v: Value = serde_json::from_str(text(a)).unwrap();
                    assert!(
                        v["conflicts"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|c| c["kind"] == "cross_org"),
                        "{who:?}: {v}"
                    );
                    assert_eq!(v["plan"]["project_id"], fx.pid_acme, "{v}");
                }
            }
        },
    )
    .await;
    // Host A's own AA-1 already runs on it: a conflict naming that session.
    let own_live = call(&fx, Who::HostA, "work_link", preview("AA-1", fx.pid_acme)).await;
    let v: Value = serde_json::from_str(text(&own_live)).unwrap_or_else(|_| panic!("{own_live:?}"));
    let live = v["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "live_session")
        .cloned()
        .unwrap_or_else(|| panic!("{v}"));
    assert_eq!(live["session_id"], fx.s_a, "{v}");

    // Agent-written handover (work graph M9.3), per caller. The sessions
    // are idle REPLs here, so a caller past the fences reaches the send
    // (which fails for want of a real host, and is recorded as such).
    //
    // * master / full client / host A: A's own s_a — a send attempt.
    // * host B: A's s_a is another host's session — it reads as unknown.
    // * host in no org: its own s_n (unassigned work) — a send attempt.
    // * readonly: refused by its mode.
    {
        let s = fx.t.store.lock().unwrap();
        s.conn_for_test()
            .execute(
                "UPDATE sessions SET claude_status = 'idle' WHERE id IN (?1, ?2, ?3)",
                [fx.s_a, fx.s_b, fx.s_n],
            )
            .unwrap();
    }
    let attempted = |fx: &Fx, who: Who, a: &Answer, sid: i64| {
        assert!(
            !matches!(
                code(a),
                "E_FORBIDDEN" | "E_NOTFOUND" | "E_INVALID" | "E_NOT_ALIVE" | "E_EXISTS"
            ),
            "{who:?}: {a:?}"
        );
        let s = fx.t.store.lock().unwrap();
        let ev = s
            .newest_session_event_of(
                sid,
                &[
                    crate::service::work::agent_handover::EV_REQUESTED,
                    crate::service::work::agent_handover::EV_SEND_FAILED,
                ],
            )
            .unwrap();
        assert!(ev.is_some(), "{who:?}: no request recorded");
    };
    // Cancelling a start (task → session P-6) kills the session, so it is
    // fenced as a kill. None of these sessions was made by a start, so a
    // caller past the fences meets the service's own refusal (`E_DIRTY`,
    // `not_a_start`) — never another host's or org's session named.
    // `Reach::Own`: a host token that does not own the session is refused
    // even on its own host, as a kill is.
    let past_fences = |who: Who, a: &Answer| {
        is_code(who, a, "E_DIRTY", "cancel a session no start made");
        assert!(format!("{a:?}").contains("not_a_start"), "{who:?}: {a:?}");
    };
    m.row(
        "work_link",
        "abandon_start",
        |fx, who| {
            let sid = match who {
                Who::HostNone => fx.s_n,
                _ => fx.s_a,
            };
            json!({ "action": "abandon_start", "session_id": sid })
        },
        move |_fx, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostB => is_code(who, a, "E_FORBIDDEN", "another host's session"),
                Who::BoundB => is_code(who, a, "E_NOTFOUND", "another org's session"),
                Who::HostA | Who::HostNone => is_code(who, a, "E_FORBIDDEN", "not its own"),
                _ => past_fences(who, a),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "handover",
        |fx, who| {
            let sid = match who {
                Who::HostB => fx.s_a,
                Who::HostNone => fx.s_n,
                _ => fx.s_a,
            };
            json!({ "action": "handover", "session_id": sid })
        },
        move |fx, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostB => is_code(who, a, "E_NOTFOUND", "another host's session"),
                Who::BoundB => is_code(who, a, "E_NOTFOUND", "another org's session"),
                Who::HostNone => attempted(fx, who, a, fx.s_n),
                _ => attempted(fx, who, a, fx.s_a),
            }
        },
    )
    .await;
    // An isolated org's session (D7): its own host still asks it; to a host
    // of another org it reads as unknown (below), isolated or not.
    let own_b = call(
        &fx,
        Who::HostB,
        "work_link",
        json!({ "action": "handover", "session_id": fx.s_b }),
    )
    .await;
    attempted(&fx, Who::HostB, &own_b, fx.s_b);
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
                // A bound client: its org's and unassigned sessions (M14).
                Who::BoundA => vec![fx.s_a, fx.s_n, fx.s_x],
                Who::BoundB => vec![fx.s_b, fx.s_n],
                _ => vec![fx.s_a, fx.s_b, fx.s_n, fx.s_x],
            };
            want.sort();
            assert_eq!(ids, want, "{who:?}: {v}");
            match who {
                Who::HostA | Who::HostNone | Who::BoundA => {
                    assert!(shipped.is_empty(), "{who:?}: {v}")
                }
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
                Who::HostB | Who::BoundB => assert_eq!(named, vec![fx.org_b]),
                Who::BoundA => assert!(!named.contains(&fx.org_b) && named.len() == 1, "{v:?}"),
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
                Who::HostA | Who::BoundA => assert!(v.iter().all(|o| o["id"] != fx.org_b)),
                Who::BoundB => assert!(v.iter().all(|o| o["id"] == fx.org_b)),
                Who::HostNone => assert!(v.is_empty()),
                _ => assert!(!v.is_empty()),
            }
            // Org administration phase A: a device's name is the fleet
            // administrator's to see — the master or the owner's unbound
            // device. A host or an org-bound device gets no `devices` key.
            let shown = matches!(who, Who::Master | Who::ClientFull | Who::ClientReadonly);
            for o in &v {
                assert_eq!(o.get("devices").is_some(), shown, "{who:?}: {o}");
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
            let key = if matches!(who, Who::HostB | Who::BoundB) {
                "AA-1"
            } else {
                "BB-1"
            };
            json!({ "action": "link", "session_id": own(fx, who), "key": key })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            if who.is_host() || who.is_bound() {
                // A key outside the caller's orgs links as the bare key it
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
        // The bare links the row just made: host A's BB-1 on s_a, host B's
        // AA-1 on s_b, and the no-org host's BB-1 on s_n.
        let bare_link = |sid: i64, key: &str| -> i64 {
            s.conn_for_test()
                .query_row(
                    "SELECT l.id FROM work_links l JOIN participants p ON p.id = l.participant_id \
                     WHERE p.session_id = ?1 AND l.ref_key = ?2 AND l.item_id IS NULL \
                       AND l.ended_at IS NULL",
                    rusqlite::params![sid, key],
                    |r| r.get(0),
                )
                .unwrap_or_else(|e| panic!("session {sid} has no bare link to {key}: {e}"))
        };
        let item_of = |link: i64| s.get_work_link(link).unwrap().unwrap().item_id;
        let (a_bb1, b_aa1, n_bb1) = (
            bare_link(fx.s_a, "BB-1"),
            bare_link(fx.s_b, "AA-1"),
            bare_link(fx.s_n, "BB-1"),
        );
        for tracker in [fx.tracker_b, fx.tracker_a] {
            s.bind_tracker_refs(tracker).unwrap();
        }
        // The control: the prefix claims the key and the unassigned host's
        // link binds — so the other two stay bare by the org fence alone.
        assert_eq!(item_of(n_bb1), Some(fx.item_b), "h-n's BB-1 binds");
        assert_eq!(item_of(a_bb1), None, "h-a's BB-1 stays bare");
        assert_eq!(item_of(b_aa1), None, "h-b's AA-1 stays bare");
        // Undo them, so later rows read the fixture as it was.
        for link in [a_bb1, b_aa1, n_bb1] {
            s.conn_for_test()
                .execute("DELETE FROM work_links WHERE id = ?1", [link])
                .unwrap();
        }
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
            let item = if matches!(who, Who::HostB | Who::BoundB) {
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
            if who.is_host() || who.is_bound() {
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
            let item = if matches!(who, Who::HostB | Who::BoundB) {
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
            if who.is_host() || who.is_bound() {
                is_code(who, a, "E_NOTFOUND", "reject another org's item");
            } else {
                is_ok(who, a, "a rejection is no link");
            }
        },
    )
    .await;
    // Link id guessing: s_x's forced link (host A's own session) and s_b's.
    // The oracle comparison comes FIRST, while link_x is live: a live link
    // the org fence hides must answer exactly as an id that does not exist
    // (compared against a missing id, both sides would read as missing and
    // the fence's own sentence would never be compared), and a refusal
    // deletes nothing.
    for action in ["confirm", "unlink"] {
        let guessed = call(
            &fx,
            Who::HostA,
            "work_link",
            json!({ "action": action, "session_id": fx.s_x, "link_id": fx.link_x }),
        )
        .await;
        let unknown = call(
            &fx,
            Who::HostA,
            "work_link",
            json!({ "action": action, "session_id": fx.s_x, "link_id": 999_999 }),
        )
        .await;
        is_code(Who::HostA, &guessed, "E_NOTFOUND", action);
        same_as_unknown(&guessed, &unknown, &fx.link_x.to_string(), "999999");
        assert!(
            link_is_live(&fx, fx.link_x),
            "{action}: a refusal deletes nothing"
        );
    }
    // The rows. An `unlink` a caller is allowed deletes the link, and the
    // callers run master first, so the link is re-made for every caller
    // (`cur` is its id for this call): each host is refused a LIVE link,
    // and every deletion is asserted rather than left to happen.
    for pick in [0, 1] {
        for action in ["confirm", "unlink"] {
            let cur = std::cell::Cell::new(0_i64);
            let cur = &cur;
            m.row(
                "work_link",
                action,
                move |fx, who| {
                    let (sid, link) = if pick == 0 {
                        let link = relink(fx, fx.s_x, fx.item_b);
                        let sid = if who.is_host() && who != Who::HostA {
                            own(fx, who)
                        } else {
                            fx.s_x
                        };
                        (sid, link)
                    } else {
                        (own(fx, who), relink(fx, fx.s_b, fx.item_b))
                    };
                    cur.set(link);
                    json!({ "action": action, "session_id": sid, "link_id": link })
                },
                move |fx, who, a| {
                    if readonly_refused(who, a) {
                        return;
                    }
                    let link = cur.get();
                    // Bound clients (M14) as hosts: B's link on A's s_x is
                    // neither A's to see nor s_x B's to reach; B's own s_b
                    // link is B's.
                    let own_b = matches!(who, Who::HostB | Who::BoundB) && pick == 1;
                    if (who.is_host() || who.is_bound()) && !own_b {
                        is_code(who, a, "E_NOTFOUND", "another org's link id");
                        assert!(link_is_live(fx, link), "{who:?}: a refusal deletes nothing");
                    } else if action == "unlink" {
                        // The link's own session: s_x for the master and the
                        // full client (pick 0), s_b for host B (pick 1). The
                        // master's and the client's pick 1 is s_a, whose
                        // links do not include link_b.
                        if pick == 0 || own_b {
                            is_ok(who, a, "unlink");
                            assert!(!link_is_live(fx, link), "{who:?}: unlinked");
                        } else {
                            is_code(who, a, "E_NOTFOUND", "not this session's link");
                            assert!(link_is_live(fx, link), "{who:?}: not deleted");
                        }
                    }
                },
            )
            .await;
        }
    }
    // The last caller of the rows above unlinked s_b's ticket: put it back.
    relink(&fx, fx.s_b, fx.item_b);
    m.row(
        "work_link",
        "trust_project",
        |fx, _| json!({ "action": "trust_project", "project_id": fx.pid_beta, "on": false }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            if who.is_host() || who.is_bound() {
                is_code(who, a, "E_FORBIDDEN", "trust is fleet configuration");
            } else {
                is_ok(who, a, "trust");
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
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "resume B's work"),
                // Everyone else gets as far as the spawn, which fails
                // here for want of a real host — never an org refusal.
                _ => assert!(!text(a).contains("not visible"), "{who:?}: {a:?}"),
            }
        },
    )
    .await;
    // Work graph M13.4c: a summary of B's past work (s-b-old on h-b). Its
    // conversation id here is not a UUID, so a caller that passes every
    // fence stops at that check, before anything runs on a host.
    let past_b: i64 = {
        let s = fx.t.store.lock().unwrap();
        s.conn_for_test()
            .query_row(
                "SELECT id FROM work_links WHERE snap_tmux = 's-b-old'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    m.row(
        "work_link",
        "summarize",
        move |_, _| json!({ "action": "summarize", "key": "BB-3", "link_id": past_b }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostA | Who::HostNone => {
                    is_code(who, a, "E_FORBIDDEN", "summarise B's past work")
                }
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "summarise B's past work"),
                _ => {
                    is_code(who, a, "E_INVALID", "past every fence");
                    assert!(text(a).contains("claude session id"), "{who:?}: {a:?}");
                }
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
                // session is the integrity refusal. B's bound client: never
                // in A's repo (M14). A's bound client: BB-2 is not its to
                // read — by URL unknown; by key a bare key in its own repo,
                // as an unknown key is (it reaches the spawn).
                match who {
                    Who::BoundA if args["url"].is_string() => {
                        is_code(who, a, "E_NOTFOUND", "B's ticket by URL")
                    }
                    Who::BoundA => assert_ne!(code(a), "E_FORBIDDEN", "{a:?}"),
                    _ => is_code(who, a, "E_FORBIDDEN", "start B's ticket on A"),
                }
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
            if matches!(who, Who::HostA | Who::HostNone | Who::BoundA) {
                is_code(who, a, "E_NOTFOUND", "start by another org's item id");
            }
        },
    )
    .await;
    // Orchestration O0: a run is a person's or the operator's — every
    // per-host token is refused before any lookup, so it answers alike for
    // its own org's item and another's; a client bound to A never finds B's.
    m.row(
        "work_link",
        "run",
        |fx, _| json!({ "action": "run", "item_id": fx.item_b, "project_id": 1 }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostA | Who::HostB | Who::HostNone => {
                    is_code(who, a, "E_FORBIDDEN", "a per-host token runs nothing")
                }
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "run another org's item"),
                // Reached: the item is found, and the start then asks what
                // every start asks of B's item in project 1 (org A): the
                // cross-org question, or B's own fence on A's project.
                _ => {
                    let said = format!("{a:?}");
                    assert!(
                        code(a) != "E_FORBIDDEN"
                            || said.contains("cross_org")
                            || (matches!(who, Who::BoundB)
                                && said.contains("only in its own org's projects")),
                        "{who:?}: {a:?}"
                    )
                }
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
                    if matches!(
                        name,
                        "list" | "list_orgs" | "status" | "sweep_now" | "usage"
                    ) {
                        is_ok(who, a, name);
                    }
                }
                _ => is_code(who, a, "E_FORBIDDEN", "work_admin is master-only"),
            },
        )
        .await;
    }

    // ── write-back (M13.4e, D3): only the master turns it on ────────────
    // No tool triggers a write (the PR probe does); the one switch is this
    // setting, and every other caller is refused before anything changes.
    m.row(
        "work_admin",
        "update",
        |fx, _| {
            json!({
                "action": "update",
                "tracker_id": fx.tracker_a,
                "settings": { "write_back": { "pr_remote_link": true } },
            })
        },
        |fx, who, a| match who {
            Who::Master => {
                is_ok(who, a, "the master turns write-back on");
                let on =
                    fx.t.store
                        .lock()
                        .unwrap()
                        .get_tracker(fx.tracker_a)
                        .unwrap()
                        .unwrap()
                        .settings
                        .write_back
                        .pr_remote_link;
                assert!(on, "the setting was stored");
            }
            _ => is_code(who, a, "E_FORBIDDEN", "write-back is the master's switch"),
        },
    )
    .await;
    // Back off, so no later row runs with a write-back tracker.
    {
        let s = fx.t.store.lock().unwrap();
        let mut t = s.get_tracker(fx.tracker_a).unwrap().unwrap();
        t.settings.write_back.pr_remote_link = false;
        s.set_tracker_settings(fx.tracker_a, &t.settings).unwrap();
    }

    // ── sessions (D7) ───────────────────────────────────────────────────
    // The rows below check what a host reads of another org's work ON A
    // SESSION ROW, so s_x (A's session carrying B's ticket) and s_b must
    // carry that work: earlier rows unlinked and re-made them.
    relink(&fx, fx.s_x, fx.item_b);
    relink(&fx, fx.s_b, fx.item_b);
    m.row(
        "list_sessions",
        "-",
        |_, _| json!({ "summary": false }),
        |fx, who, a| {
            let rows: Vec<Value> = serde_json::from_str(text(a)).unwrap();
            let ids: BTreeSet<i64> = rows.iter().filter_map(|r| r["id"].as_i64()).collect();
            // Choke point 1 (multi-user M1, T6). An invisible row is
            // DROPPED, so the page itself is the assertion — a caller
            // cannot learn that a row it may not see exists.
            //
            // D7's `isolate_sessions` no longer appears on either side of
            // this: the host fence hides every other host's rows whatever
            // any org says, so both runs of this matrix expect the same
            // page here.
            let want: Vec<i64> = match who {
                // §4.4's two clauses: the one row this token's pane proves,
                // plus the `unclaimed` rows on its own host. Not another
                // person's private row (s_b, s_n), not an unassigned row on
                // another host, not an org-mate's row on another host.
                Who::HostA => vec![fx.s_a, fx.s_x],
                Who::HostB => vec![fx.s_b],
                Who::HostNone => vec![fx.s_n],
                // A client bound to an org is the same person, fenced by
                // the org half: its org's sessions plus the unassigned one
                // (D31 on).
                Who::BoundA => vec![fx.s_a, fx.s_x, fx.s_n],
                Who::BoundB => vec![fx.s_b, fx.s_n],
                // The one person on this hub: the three rows they own, and
                // the `unclaimed` one they could already see before M1.
                _ => vec![fx.s_a, fx.s_b, fx.s_n, fx.s_x],
            };
            assert_eq!(
                ids,
                want.into_iter().collect::<BTreeSet<_>>(),
                "{who:?} listed the wrong rows"
            );
            // The work on a row: the master and the clients read all of it,
            // a host only its own org's — B's ticket on A's own s_x is
            // stripped for host A, and the no-org host reads only its own
            // unassigned work.
            let work_key = |id: i64| {
                let r = rows
                    .iter()
                    .find(|r| r["id"] == id)
                    .unwrap_or_else(|| panic!("{who:?} lists session {id}"));
                r["work"]["key"].as_str().map(str::to_string)
            };
            match who {
                Who::HostA | Who::BoundA => {
                    assert_eq!(work_key(fx.s_a).as_deref(), Some("AA-1"));
                    assert_eq!(work_key(fx.s_x), None, "B's ticket on A's session");
                }
                Who::HostB | Who::BoundB => {
                    assert_eq!(work_key(fx.s_b).as_deref(), Some("BB-1"));
                }
                Who::HostNone => {
                    assert_eq!(
                        work_key(fx.s_n).as_deref(),
                        Some("LOC-1"),
                        "its own unassigned work is not another org's"
                    );
                }
                _ => {
                    assert_eq!(work_key(fx.s_a).as_deref(), Some("AA-1"));
                    assert_eq!(work_key(fx.s_x).as_deref(), Some("BB-1"));
                    assert_eq!(work_key(fx.s_b).as_deref(), Some("BB-1"));
                }
            }
        },
    )
    .await;
    m.row(
        "peer_status",
        "-",
        |fx, _| json!({ "session_id": fx.s_b }),
        |_, who, a| match who {
            // Multi-user M1 (T6): a per-host token reads its own host's
            // sessions and nothing else, so s_b (on h-b) is out of reach
            // for both of these whether or not org B isolates.
            Who::HostA | Who::HostNone => is_code(who, a, "E_NOTFOUND", "another host's session"),
            Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's session"),
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
        |_, who, a| match who {
            // An anchor the caller may not see answers exactly as a missing
            // one — and since M1 that is every other host's row.
            // `E_NOTFOUND`, in `orgs::not_found`'s one sentence: the anchor
            // check answers invisible and missing alike now, where it used to
            // leak `rusqlite`'s own "Query returned no rows" for the missing
            // half and nothing at all for the invisible one (multi-user M1,
            // T7 — the check was behind `if !scope.is_all()`).
            Who::HostA | Who::HostNone => is_code(who, a, "E_NOTFOUND", "another host's anchor"),
            Who::BoundA => assert!(a.is_err(), "another org's anchor: {a:?}"),
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
        // The one sentence with the id swapped, as everywhere else: both are
        // `orgs::not_found("session", …)` now, so the ONLY difference between
        // "a row you may not see" and "no such row" is the number the caller
        // itself passed in.
        same_as_unknown(&hidden, &missing, &fx.s_b.to_string(), "999999");
    }
    m.row(
        "send_message",
        "-",
        |fx, who| {
            json!({ "from_session_id": own(fx, who), "to_session_id": fx.s_b,
                    "body": "hello", "deliver": false })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostA | Who::HostNone => {
                    is_code(who, a, "E_NOTFOUND", "message to another host's session")
                }
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "message to another org's session"),
                Who::HostB | Who::BoundB => is_code(who, a, "E_SELF_TARGET", "own session"),
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
    // an isolated org's session reads as missing. `whoami` answers the row
    // itself, so B's work on it is read by the master, the full client
    // (whoami is not a readonly tool) and B's own host — and by nobody else
    // (the leak check above).
    assert!(link_is_live(&fx, relink(&fx, fx.s_b, fx.item_b)));
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
            // M1: another host's session is simply not there, for every
            // per-host token that is not standing on it.
            if matches!(who, Who::HostA | Who::HostNone) {
                is_code(who, &a, "E_NOTFOUND", tool);
            }
            // A client bound to A never reaches B's session (M14).
            if who == Who::BoundA {
                is_code(who, &a, "E_NOTFOUND", tool);
            }
            if tool == "whoami"
                && matches!(
                    who,
                    Who::Master | Who::ClientFull | Who::HostB | Who::BoundB
                )
            {
                is_ok(who, &a, tool);
                let row: Value = serde_json::from_str(text(&a)).unwrap();
                assert_eq!(row["work"]["key"], "BB-1", "{who:?}: {a:?}");
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
    // The frames carry the work the fence must strip: s_x's and s_b's is
    // B's, s_a's is A's.
    let key_of = |r: &crate::store::SessionRow| r.work.as_ref().and_then(|w| w.key.clone());
    for (id, key) in [(fx.s_x, "BB-1"), (fx.s_b, "BB-1"), (fx.s_a, "AA-1")] {
        let r = rows.iter().find(|r| r.id == id).unwrap();
        assert_eq!(key_of(r).as_deref(), Some(key), "fixture: session {id}");
    }
    for &who in EVERYONE {
        let c = who.caller();
        // Multi-user M1 (T9): the stream fence takes the whole `ViewScope`,
        // not the org half — so this is the same value a live stream holds.
        let scope = {
            let s = fx.t.store.lock().unwrap();
            c.view_scope(&s).unwrap()
        };
        let kinds = crate::mcp::events_route::fence_host_bound(&c, None);
        if who.is_host() || who.is_bound() {
            assert!(
                !kinds.unwrap().iter().any(|k| k == "work"),
                "work frames never reach a host nor a bound client"
            );
        }
        for (i, row) in rows.iter().enumerate() {
            let msg = crate::events::EventMessage {
                name: "session:updated",
                payload: serde_json::to_value(row).unwrap(),
                seq: i as u64 + 1,
            };
            // The same session's timeline frame (no row, only its id).
            let ev = crate::events::EventMessage {
                name: "session:event",
                payload: json!({ "id": 1, "session_id": row.id, "kind": "prompt_sent" }),
                seq: 100 + i as u64,
            };
            // …and its kill frame, which carries the row's facts because by
            // the time one is read the row is gone (T9).
            let killed = crate::events::EventMessage {
                name: "session:killed",
                payload: crate::events::RowChange::SessionKilled(
                    crate::events::SessionKilledPayload::of_row(row),
                )
                .payload(),
                seq: 200 + i as u64,
            };
            let ev_out = crate::mcp::events_route::fence_frame(&scope, &ev, &fx.t.store);
            let out = crate::mcp::events_route::fence_frame(&scope, &msg, &fx.t.store);
            let killed_out = crate::mcp::events_route::fence_frame(&scope, &killed, &fx.t.store);
            assert_eq!(
                ev_out.is_none(),
                out.is_none(),
                "{who:?}: a row and its events agree"
            );
            assert_eq!(
                killed_out.is_none(),
                out.is_none(),
                "{who:?}: a row and its kill frame agree"
            );
            let is_b = row.id == fx.s_b;
            let is_a = row.id == fx.s_a || row.id == fx.s_x;
            // Multi-user M1 (T6): a per-host token's stream carries its own
            // host's frames and no others — D7 no longer decides it, and
            // neither run of this matrix differs here. A bound client never
            // reads another org's session frame (M14), isolated or not.
            let dropped = match who {
                Who::HostA => row.host_alias != "h-a",
                Who::HostB => row.host_alias != "h-b",
                Who::HostNone => row.host_alias != "h-n",
                Who::BoundA => is_b,
                Who::BoundB => is_a,
                _ => false,
            };
            if dropped {
                assert!(out.is_none(), "{who:?} got an isolated frame of {}", row.id);
                continue;
            }
            let kept = out.expect("frame kept");
            // The row's work in the frame: whole for the master and the
            // clients, B's for B's host, stripped from A's session for A's
            // host, stripped everywhere for the no-org host.
            let own_key = key_of(row);
            let expected: Option<&str> = match (who, row.id) {
                (Who::HostA | Who::BoundA, id) if id == fx.s_a => Some("AA-1"),
                (Who::HostB | Who::BoundB, id) if id == fx.s_b => Some("BB-1"),
                (Who::HostA | Who::HostB | Who::HostNone | Who::BoundA | Who::BoundB, _) => None,
                _ => own_key.as_deref(),
            };
            if row.id != fx.s_n {
                assert_eq!(
                    kept["work"]["key"].as_str(),
                    expected,
                    "{who:?} on session {}",
                    row.id
                );
            }
            let out = kept.to_string();
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
                api: None,
                host_alias: Some(host.into()),
                client: None,
                mode: TokenMode::Full,
                pane: None,
                is_personal_owner: false,
            };
            let ctx = crate::service::hooks::HookContext {
                caller: &c,
                pane_id: None,
                sync_start: true,
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
                Who::HostA | Who::BoundA => assert!(has(fx.s_x) && !has(fx.s_b), "{t}"),
                Who::HostB | Who::BoundB => assert!(has(fx.s_b) && !has(fx.s_x), "{t}"),
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
                matches!(who, Who::HostA | Who::HostNone | Who::BoundA),
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
                Who::HostA | Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's link"),
                Who::BoundB => is_code(who, a, "E_NOTFOUND", "another org's session"),
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
            if who.is_host() || who.is_bound() {
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

    // ── local work items (work graph M11.1) ────────────────────────────
    // Name new work on the caller's own session: everyone but the readonly
    // client (its mode refuses `work_link`).
    m.row(
        "work_link",
        "name",
        |fx, who| {
            json!({ "action": "name", "session_id": own(fx, who),
                    "title": format!("named-{who:?}") })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            is_ok(who, a, "name own session's work");
        },
    )
    .await;
    // Another host's session reads as a session that does not exist.
    let unknown_session = call(
        &fx,
        Who::HostA,
        "work_link",
        json!({ "action": "name", "session_id": 999_999, "title": "t" }),
    )
    .await;
    m.row(
        "work_link",
        "name",
        |fx, _| json!({ "action": "name", "session_id": fx.s_b, "title": "t" }),
        |fx, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostA | Who::HostNone | Who::BoundA => {
                    same_as_unknown(a, &unknown_session, &fx.s_b.to_string(), "999999")
                }
                _ => is_ok(who, a, "name s_b's work"),
            }
        },
    )
    .await;
    // Local keys the rows below named, taken out again so a later row's
    // leak check does not read host A's own key as B's.
    let drop_local = |keys: &str| {
        let s = fx.t.store.lock().unwrap();
        s.conn_for_test()
            .execute_batch(&format!(
                "DELETE FROM work_links WHERE item_id IN (SELECT id FROM work_items \
                   WHERE source = 'local' AND key IN ({keys})); \
                 DELETE FROM work_items WHERE source = 'local' AND key IN ({keys});"
            ))
            .unwrap();
    };
    // A key B's tracker carries is no oracle for host A: it names work as
    // an unknown key does.
    for key in ["BB-1", "ZZ-77"] {
        let named = call(
            &fx,
            Who::HostA,
            "work_link",
            json!({ "action": "name", "session_id": fx.s_a, "title": "a-local", "key": key }),
        )
        .await;
        is_ok(Who::HostA, &named, key);
    }
    drop_local("'BB-1', 'ZZ-77'");
    // A taken key: refused for whoever sees B's ticket; host A names its
    // own work under it (as above), and then host N meets that local item
    // (local keys are one fleet-wide namespace).
    m.row(
        "work_link",
        "name",
        |fx, who| {
            json!({ "action": "name", "session_id": own(fx, who), "title": "dup",
                    "key": "BB-1" })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                // Bound A comes after host A, and meets the local item host
                // A just named under that key (one fleet-wide namespace).
                Who::HostA => is_ok(who, a, "an invisible ticket's key"),
                _ => is_code(who, a, "E_EXISTS", "a taken key"),
            }
        },
    )
    .await;
    drop_local("'BB-1'");
    // Rename: host A's local item, hidden from the other hosts.
    let local_a = {
        let s = fx.t.store.lock().unwrap();
        s.name_session_work(fx.s_a, Some("LOC-A"), "local-a")
            .unwrap()
            .0
            .id
    };
    let unknown_item = call(
        &fx,
        Who::HostB,
        "work_link",
        json!({ "action": "name", "item_id": 999_999, "title": "t" }),
    )
    .await;
    m.row(
        "work_link",
        "name",
        move |_, who| json!({ "action": "name", "item_id": local_a, "title": format!("renamed-{who:?}") }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostB | Who::HostNone | Who::BoundB => {
                    same_as_unknown(a, &unknown_item, &local_a.to_string(), "999999")
                }
                _ => assert!(text(a).contains(&format!("renamed-{who:?}")), "{who:?}: {a:?}"),
            }
        },
    )
    .await;
    // A ticket is never renamed: refused for who sees it, unknown otherwise.
    m.row(
        "work_link",
        "name",
        |fx, _| json!({ "action": "name", "item_id": fx.item_b, "title": "t" }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostA | Who::HostNone | Who::BoundA => {
                    is_code(who, a, "E_NOTFOUND", "another org's ticket")
                }
                _ => is_code(who, a, "E_INVALID", "a ticket"),
            }
        },
    )
    .await;
    // Status (task 2, native item status): host A's own local item, set by
    // whoever may see it; another host's answers as unknown — the same
    // fence rename uses.
    let unknown_item_status = call(
        &fx,
        Who::HostB,
        "work_link",
        json!({ "action": "set_status", "item_id": 999_999, "status": "done" }),
    )
    .await;
    m.row(
        "work_link",
        "set_status",
        move |_, _| json!({ "action": "set_status", "item_id": local_a, "status": "done" }),
        move |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostB | Who::HostNone | Who::BoundB => {
                    same_as_unknown(a, &unknown_item_status, &local_a.to_string(), "999999")
                }
                _ => assert!(
                    text(a).contains("\"status_category\":\"done\""),
                    "{who:?}: {a:?}"
                ),
            }
        },
    )
    .await;
    // A ticket's status is not a person's to set here: refused for who
    // sees it, unknown otherwise (`store::tracker_items` owns it).
    m.row(
        "work_link",
        "set_status",
        |fx, _| json!({ "action": "set_status", "item_id": fx.item_b, "status": "done" }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostA | Who::HostNone | Who::BoundA => {
                    is_code(who, a, "E_NOTFOUND", "another org's ticket")
                }
                _ => is_code(who, a, "E_INVALID", "a ticket"),
            }
        },
    )
    .await;
    // Task editing: `set_status`'s fences — host A's own local item edited
    // by whoever may see it, another host's answering as unknown, a
    // ticket refused for who sees it.
    let unknown_item_edit = call(
        &fx,
        Who::HostB,
        "work_link",
        json!({ "action": "edit", "item_id": 999_999, "notes": "n" }),
    )
    .await;
    m.row(
        "work_link",
        "edit",
        move |_, _| json!({ "action": "edit", "item_id": local_a, "notes": "edited notes" }),
        move |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostB | Who::HostNone | Who::BoundB => {
                    same_as_unknown(a, &unknown_item_edit, &local_a.to_string(), "999999")
                }
                _ => assert!(
                    text(a).contains("\"notes\":\"edited notes\""),
                    "{who:?}: {a:?}"
                ),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "edit",
        |fx, _| json!({ "action": "edit", "item_id": fx.item_b, "title": "Hijack" }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostA | Who::HostNone | Who::BoundA => {
                    is_code(who, a, "E_NOTFOUND", "another org's ticket")
                }
                _ => is_code(who, a, "E_INVALID", "a ticket"),
            }
        },
    )
    .await;
    // Sprints and releases (design 2026-09-28 §7, §8): fenced by the
    // bucket's own org; a member by its item's; membership is a person's
    // plan, never a session's.
    let (bucket_a, bucket_b) = {
        let s = fx.t.store.lock().unwrap();
        let make = |name: &str, org: i64| {
            s.create_bucket(&crate::store::NewBucket {
                kind: "sprint",
                name,
                org_id: Some(org),
                ..Default::default()
            })
            .unwrap()
            .id
        };
        let a = make("Alpha sprint", ORG_A);
        let b = make("Bravo sprint", fx.org_b);
        s.add_bucket_item(b, fx.item_b).unwrap();
        (a, b)
    };
    m.row(
        "work",
        "buckets",
        |_, _| json!({ "action": "buckets" }),
        move |_, who, a| {
            is_ok(who, a, "buckets");
            let t = text(a);
            let sees = |id: i64| t.contains(&format!("\"id\":{id},"));
            match who {
                Who::HostA | Who::BoundA => {
                    assert!(sees(bucket_a) && !sees(bucket_b), "{who:?}: {t}")
                }
                Who::HostB | Who::BoundB => {
                    assert!(sees(bucket_b) && !sees(bucket_a), "{who:?}: {t}")
                }
                Who::HostNone => assert!(!sees(bucket_a) && !sees(bucket_b), "{t}"),
                _ => assert!(sees(bucket_a) && sees(bucket_b), "{who:?}: {t}"),
            }
        },
    )
    .await;
    let unknown_bucket = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "bucket", "bucket_id": 999_999 }),
    )
    .await;
    m.row(
        "work",
        "bucket",
        move |_, _| json!({ "action": "bucket", "bucket_id": bucket_b }),
        move |fx, who, a| match who {
            Who::HostA | Who::HostNone | Who::BoundA => {
                same_as_unknown(a, &unknown_bucket, &bucket_b.to_string(), "999999")
            }
            _ => {
                is_ok(who, a, "B's sprint");
                assert!(
                    text(a).contains(&format!("\"id\":{},", fx.item_b)),
                    "{who:?}: the member is listed: {a:?}"
                );
            }
        },
    )
    .await;
    for action in ["bucket_add", "bucket_remove"] {
        m.row(
            "work_link",
            action,
            move |fx, _| json!({ "action": action, "bucket_id": bucket_b, "item_id": fx.item_b }),
            move |_, who, a| {
                if readonly_refused(who, a) {
                    return;
                }
                match who {
                    w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not plan"),
                    Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's sprint"),
                    Who::Master => is_ok(who, a, action),
                    _ => {}
                }
            },
        )
        .await;
    }
    // An A bucket with B's ticket: B's bound client cannot reach the
    // bucket, A's cannot reach the ticket — each as unknown.
    m.row(
        "work_link",
        "bucket_add",
        move |fx, _| json!({ "action": "bucket_add", "bucket_id": bucket_a, "item_id": fx.item_b }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::BoundA | Who::BoundB => is_code(who, a, "E_NOTFOUND", "outside the org"),
                // Visible to both: the store refuses the cross-org plan.
                Who::Master => is_code(who, a, "E_FORBIDDEN", "cross-org"),
                _ => {}
            }
        },
    )
    .await;
    // Missions (orchestration O1): fenced by the mission's own org, then by
    // its owner or the org's members. Every client here is the hub's one
    // person, who owns both; a per-host token proves no person and sees
    // none, and never writes one.
    let (mission_a, mission_b) = {
        let s = fx.t.store.lock().unwrap();
        let make = |name: &str, org: i64| {
            s.create_mission(
                &crate::store::NewMission {
                    org_id: Some(org),
                    owner_person_id: Some(PERSON),
                    name,
                    goal: "the goal",
                    ..Default::default()
                },
                "person:1",
            )
            .unwrap()
            .id
        };
        (
            make("Alpha mission", ORG_A),
            make("Bravo mission", fx.org_b),
        )
    };
    m.row(
        "work",
        "missions",
        |_, _| json!({ "action": "missions" }),
        move |_, who, a| {
            is_ok(who, a, "missions");
            let t = text(a);
            let sees = |id: i64| t.contains(&format!("\"id\":{id},"));
            match who {
                Who::BoundA => assert!(sees(mission_a) && !sees(mission_b), "{t}"),
                Who::BoundB => assert!(sees(mission_b) && !sees(mission_a), "{t}"),
                w if w.is_host() => assert!(!sees(mission_a) && !sees(mission_b), "{t}"),
                _ => assert!(sees(mission_a) && sees(mission_b), "{who:?}: {t}"),
            }
        },
    )
    .await;
    let unknown_mission = call(
        &fx,
        Who::HostA,
        "work",
        json!({ "action": "mission", "mission_id": 999_999 }),
    )
    .await;
    m.row(
        "work",
        "mission",
        move |_, _| json!({ "action": "mission", "mission_id": mission_b }),
        move |_, who, a| match who {
            Who::HostA | Who::HostB | Who::HostNone | Who::BoundA => {
                same_as_unknown(a, &unknown_mission, &mission_b.to_string(), "999999")
            }
            _ => {
                is_ok(who, a, "B's mission");
                assert!(text(a).contains("Bravo mission"), "{who:?}: {a:?}");
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "mission_save",
        |_, _| {
            json!({ "action": "mission_save",
                    "mission": { "name": "New", "goal": "g", "org_id": ORG_B } })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org"),
                Who::BoundB => is_code(who, a, "E_FORBIDDEN", "a root needs an item"),
                _ => is_ok(who, a, "mission_save"),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "mission_state",
        move |_, _| json!({ "action": "mission_state", "mission_id": mission_b, "status": "active" }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's mission"),
                _ => is_ok(who, a, "mission_state"),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "mission_repo",
        move |fx, _| {
            json!({ "action": "mission_repo", "mission_id": mission_b,
                    "project_id": fx.pid_beta })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's mission"),
                _ => is_ok(who, a, "mission_repo"),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "mission_item",
        move |fx, _| {
            json!({ "action": "mission_item", "mission_id": mission_b, "item_id": fx.item_b })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's mission"),
                _ => is_ok(who, a, "mission_item"),
            }
        },
    )
    .await;
    // An A mission with B's ticket: each bound client misses one of the two
    // as unknown; a caller who sees both is refused the cross-org member.
    m.row(
        "work_link",
        "mission_item",
        move |fx, _| {
            json!({ "action": "mission_item", "mission_id": mission_a, "item_id": fx.item_b })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundA | Who::BoundB => is_code(who, a, "E_NOTFOUND", "outside the org"),
                _ => is_code(who, a, "E_FORBIDDEN", "cross-org"),
            }
        },
    )
    .await;
    // A plan's steps into B's mission: tasks under its root, for whoever
    // may change it; unknown to whoever may not see it.
    m.row(
        "work_link",
        "mission_import",
        move |_, _| {
            json!({ "action": "mission_import", "mission_id": mission_b,
                    "plan": [{ "step": "1.1", "title": "Schema", "lane": "A" }] })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's mission"),
                _ => is_ok(who, a, "mission_import"),
            }
        },
    )
    .await;
    // B's mission is active by now: deleting it is refused to whoever may
    // change it, and unknown to whoever may not see it.
    m.row(
        "work_link",
        "mission_delete",
        move |_, _| json!({ "action": "mission_delete", "mission_id": mission_b }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's mission"),
                _ => is_code(who, a, "E_INVALID", "cancel it first"),
            }
        },
    )
    .await;
    m.row(
        "work",
        "local_items",
        |_, _| json!({ "action": "local_items" }),
        move |_, who, a| {
            is_ok(who, a, "local_items");
            let t = text(a);
            let lists_a = t.contains(&format!("\"id\":{local_a},"));
            match who {
                Who::HostB => {
                    assert!(!lists_a && t.contains("named-HostB"), "{t}");
                    assert!(!t.contains("named-HostA"), "{t}");
                }
                Who::HostNone => assert!(!lists_a && !t.contains("named-HostA"), "{t}"),
                Who::HostA => assert!(lists_a && !t.contains("named-HostB"), "{t}"),
                // A bound client: its org's local work, no host fence (M14).
                Who::BoundA => assert!(
                    lists_a
                        && t.contains("named-HostA")
                        && !t.contains("named-HostB")
                        && !t.contains("named-BoundB"),
                    "{t}"
                ),
                Who::BoundB => assert!(
                    !lists_a && t.contains("named-HostB") && !t.contains("named-HostA"),
                    "{t}"
                ),
                _ => assert!(lists_a && t.contains("named-HostB"), "{who:?}: {t}"),
            }
        },
    )
    .await;
    // ── Shared work context (design 2026-09-29) ────────────────────────
    // A standalone task needs an unscoped caller: a new item has no links,
    // and a scoped caller sees a local item only through its links.
    m.row(
        "work_link",
        "create",
        |_, _| json!({ "action": "create", "title": "matrix task" }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::Master | Who::ClientFull => {
                    assert!(text(a).contains("\"origin\":\"manual\""), "{who:?}: {a:?}")
                }
                _ => is_code(
                    who,
                    a,
                    "E_FORBIDDEN",
                    "a standalone task needs an unscoped caller",
                ),
            }
        },
    )
    .await;
    // A subtask under host A's local item: whoever sees the item may add
    // one; another host's (or org's) caller reads it as an unknown id.
    let unknown_parent = call(
        &fx,
        Who::HostB,
        "work_link",
        json!({ "action": "create", "parent": "item:999999", "title": "t" }),
    )
    .await;
    m.row(
        "work_link",
        "create",
        move |_, _| json!({ "action": "create", "parent": format!("item:{local_a}"), "title": "matrix step" }),
        move |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostB | Who::HostNone | Who::BoundB => {
                    same_as_unknown(a, &unknown_parent, &local_a.to_string(), "999999")
                }
                _ => assert!(
                    text(a).contains(&format!("\"parent_id\":{local_a}")),
                    "{who:?}: {a:?}"
                ),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "propose",
        move |_, _| {
            json!({ "action": "propose", "parent": format!("item:{local_a}"),
                    "title": "matrix idea", "why": "x" })
        },
        move |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostB | Who::HostNone | Who::BoundB => {
                    is_code(who, a, "E_NOTFOUND", "another org's parent")
                }
                _ => assert!(
                    text(a).contains("\"proposal_state\":\"proposed\""),
                    "{who:?}: {a:?}"
                ),
            }
        },
    )
    .await;
    // A person decides: never a per-host token or a bound client.
    for action in ["accept", "reject"] {
        m.row(
            "work_link",
            action,
            move |_, _| json!({ "action": action, "item_id": 999_999 }),
            |_, who, a| {
                if readonly_refused(who, a) {
                    return;
                }
                match who {
                    Who::Master | Who::ClientFull => {
                        is_code(who, a, "E_NOTFOUND", "unknown proposal")
                    }
                    _ => is_code(who, a, "E_FORBIDDEN", "a person decides"),
                }
            },
        )
        .await;
    }
    // ── The mission graph (orchestration O2) ───────────────────────────
    // A tree is `propose`'s parent question; an edge and a hold are a
    // person's plan; many decisions at once are still a person's.
    m.row(
        "work_link",
        "propose_tree",
        move |_, _| {
            json!({ "action": "propose_tree", "parent": format!("item:{local_a}"),
                    "tree": [{ "title": "matrix plan" }] })
        },
        move |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::HostB | Who::HostNone | Who::BoundB => {
                    is_code(who, a, "E_NOTFOUND", "another org's parent")
                }
                _ => assert!(
                    text(a).contains("\"proposal_state\":\"proposed\""),
                    "{who:?}: {a:?}"
                ),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "dep",
        move |fx, _| json!({ "action": "dep", "item_id": fx.item_b, "depends_on": fx.item_a }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not plan"),
                Who::BoundA | Who::BoundB => is_code(who, a, "E_NOTFOUND", "another org's item"),
                // Both visible: the store refuses an edge across orgs.
                _ => is_code(who, a, "E_FORBIDDEN", "cross-org edge"),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "hold",
        move |fx, _| json!({ "action": "hold", "item_id": fx.item_b }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not plan"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's item"),
                _ => is_ok(who, a, "hold"),
            }
        },
    )
    .await;
    for action in ["accept_many", "undo_accept"] {
        m.row(
            "work_link",
            action,
            move |fx, _| json!({ "action": action, "item_ids": [fx.item_b] }),
            |_, who, a| {
                if readonly_refused(who, a) {
                    return;
                }
                match who {
                    // A ticket is no proposal: the store's answer, once the
                    // caller is a person who may see it.
                    Who::Master | Who::ClientFull => is_code(who, a, "E_INVALID", "not a proposal"),
                    _ => is_code(who, a, "E_FORBIDDEN", "a person decides"),
                }
            },
        )
        .await;
    }
    // ── Acceptance conditions (orchestration O3) ───────────────────────
    // Setting them is a person's plan, as an edge is; recording a check is a
    // person's decision, as accepting is.
    m.row(
        "work_link",
        "done_when",
        move |fx, _| json!({ "action": "done_when", "item_id": fx.item_b, "done_when": ["person"] }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not plan"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's item"),
                _ => is_ok(who, a, "done_when"),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "verify",
        move |fx, _| {
            json!({ "action": "verify", "item_id": fx.item_b, "line": "matrix: never a line", "ok": true })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                // The person reaches the item; the line is not one of its.
                Who::Master | Who::ClientFull => is_code(who, a, "E_INVALID", "not its line"),
                _ => is_code(who, a, "E_FORBIDDEN", "a person decides"),
            }
        },
    )
    .await;
    // ── The mission loop (orchestration O4–O6) ─────────────────────────
    // Taking a step, asking the planner and ending a grant are a person's
    // plan, fenced by the mission; signing a grant and deciding a card are a
    // person's decision, refused to every scoped caller before any row.
    m.row(
        "work_link",
        "mission_start",
        move |_, _| json!({ "action": "mission_start", "mission_id": mission_b, "step": "run:999999" }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's mission"),
                _ => is_code(who, a, "E_INVALID_STATE", "not a next step"),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "mission_grant",
        move |_, _| json!({ "action": "mission_grant", "mission_id": mission_b, "level": 2, "hours": 1 }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::Master | Who::ClientFull => is_ok(who, a, "mission_grant"),
                _ => is_code(who, a, "E_FORBIDDEN", "a person decides"),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "mission_revoke",
        move |_, _| json!({ "action": "mission_revoke", "mission_id": mission_b }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's mission"),
                _ => is_ok(who, a, "mission_revoke"),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "card_decide",
        |_, _| json!({ "action": "card_decide", "card_id": 999_999, "ok": true }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::Master | Who::ClientFull => is_code(who, a, "E_NOTFOUND", "no such card"),
                _ => is_code(who, a, "E_FORBIDDEN", "a person decides"),
            }
        },
    )
    .await;
    // A's ticket is in no mission: the retry is refused once the item is
    // seen, and the item is unknown to whoever may not see it.
    m.row(
        "work_link",
        "retry",
        |fx, _| json!({ "action": "retry", "item_id": fx.item_a }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundB => is_code(who, a, "E_NOTFOUND", "another org's item"),
                _ => is_code(who, a, "E_INVALID_STATE", "in no mission"),
            }
        },
    )
    .await;
    // B's mission has no worker yet, so its planner has no host to run on.
    m.row(
        "work_link",
        "mission_plan",
        move |_, _| json!({ "action": "mission_plan", "mission_id": mission_b }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's mission"),
                _ => is_code(who, a, "E_INVALID_STATE", "no host for the planner"),
            }
        },
    )
    .await;
    // Redesign 9.11: B's mission is not completed, so whoever may change it
    // is told a release note waits for that; nothing runs.
    m.row(
        "work_link",
        "mission_release_note",
        move |_, _| json!({ "action": "mission_release_note", "mission_id": mission_b }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's mission"),
                _ => is_code(
                    who,
                    a,
                    "E_INVALID_STATE",
                    "a release note is for a completed mission",
                ),
            }
        },
    )
    .await;
    // Without refresh the brief answers the one drafted last and runs
    // nothing; each caller's is their own.
    m.row(
        "work_link",
        "today_brief",
        |_, _| json!({ "action": "today_brief" }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                _ => is_ok(who, a, "today_brief"),
            }
        },
    )
    .await;
    // Redesign 9.10: B's mission is not stuck, so whoever sees it gets an
    // empty card; triage changes nothing either way.
    m.row(
        "work_link",
        "mission_triage",
        move |_, _| json!({ "action": "mission_triage", "mission_id": mission_b }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                Who::BoundA => is_code(who, a, "E_NOTFOUND", "another org's mission"),
                _ => is_ok(who, a, "mission_triage"),
            }
        },
    )
    .await;
    // Last of the loop's rows: it pauses B's mission for the rows after.
    m.row(
        "work_link",
        "missions_pause_all",
        |_, _| json!({ "action": "missions_pause_all" }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "a session does not run"),
                _ => is_ok(who, a, "missions_pause_all"),
            }
        },
    )
    .await;
    // ── idle_unlinked and keep (work graph M11.3) ──────────────────────
    // Two work sessions with their own worktrees and no work linked, idle
    // and unprompted forever: A's on h-a, B's on h-b.
    let (u_a, u_b) = {
        let s = fx.t.store.lock().unwrap();
        let unlinked = |name: &str, host: &str, pid: i64| {
            let wid = s
                .upsert_worktree_on(host, pid, name, &format!("/src/{name}"), Some(name))
                .unwrap();
            let id = s
                .upsert_session(name, host, Some(pid), Some(wid), 1, 1, "running", None)
                .unwrap();
            s.conn_for_test()
                .execute_batch(&format!(
                    "UPDATE sessions SET claude_status = 'idle', idle_since = 0, \
                       created_at = 0, worktree_key = '{name}' WHERE id = {id};"
                ))
                .unwrap();
            id
        };
        (
            unlinked("u-a", "h-a", fx.pid_acme),
            unlinked("u-b", "h-b", fx.pid_beta),
        )
    };
    m.row(
        "work",
        "tidy",
        |_, _| json!({ "action": "tidy" }),
        move |_, who, a| {
            is_ok(who, a, "tidy");
            let t = text(a);
            let v: Value = serde_json::from_str(t).unwrap();
            let has =
                |id: i64| {
                    v["candidates"].as_array().unwrap().iter().any(|c| {
                        c["session_id"] == json!(id) && c["reason"] == json!("idle_unlinked")
                    })
                };
            match who {
                Who::HostA | Who::BoundA => assert!(has(u_a) && !has(u_b), "{t}"),
                Who::HostB | Who::BoundB => assert!(has(u_b) && !has(u_a), "{t}"),
                Who::HostNone => assert!(!has(u_a) && !has(u_b), "{t}"),
                _ => assert!(has(u_a) && has(u_b), "{who:?}: {t}"),
            }
        },
    )
    .await;
    // Keep is per session, fenced like every tidy item: another host's or
    // org's session reads as one that does not exist.
    m.row(
        "work_link",
        "tidy_apply",
        move |_, _| {
            json!({ "action": "tidy_apply",
                    "items": [{ "session_id": u_b, "action": "keep", "days": 3 }] })
        },
        move |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            is_ok(who, a, "a batch always answers");
            let t = text(a);
            let refused = t.contains(&format!("session {u_b} not found"));
            assert_eq!(
                refused,
                matches!(who, Who::HostA | Who::HostNone | Who::BoundA),
                "{who:?}: {t}"
            );
            assert_eq!(t.contains("\"outcome\":\"kept\""), !refused, "{who:?}: {t}");
        },
    )
    .await;
    {
        let s = fx.t.store.lock().unwrap();
        for id in [u_a, u_b] {
            s.delete_session(id).unwrap();
        }
    }

    // ── the Work view's reads (work graph M14.1b) ───────────────────────
    // Navigation a person keeps (M14.1c writes it; seeded here): a rule on
    // each org's tracker, one on a word of A's title, and a saved view of
    // every owner. Their names are markers of their org.
    let bravo_rule;
    {
        let s = fx.t.store.lock().unwrap();
        let rule =
            |name: &str, c: crate::store::RuleConditions, group: &str| s.seed_rule(name, &c, group);
        let _ = rule(
            "Alpha rule",
            crate::store::RuleConditions {
                tracker_id: Some(fx.tracker_a),
                ..Default::default()
            },
            "SECRET-A group",
        );
        bravo_rule = rule(
            "Bravo rule",
            crate::store::RuleConditions {
                tracker_id: Some(fx.tracker_b),
                ..Default::default()
            },
            "SECRET-B group",
        );
        let _ = rule(
            "Alpha login rule",
            crate::store::RuleConditions {
                title_contains: Some("login".into()),
                ..Default::default()
            },
            "SECRET-A login",
        );
        s.seed_view("Everyone's", &json!({}), None);
        s.seed_view("SECRET-A view", &json!({ "org": ORG_A }), Some(ORG_A));
        s.seed_view("SECRET-B view", &json!({ "org": ORG_B }), Some(ORG_B));
    }
    // The tree: each caller its own orgs' tasks; the forced cross-org link
    // (s_x, an A session, on B's BB-1) never names s_x to an org-B reader
    // nor BB-1 to an org-A one. Archived tasks included (BB-3 is done with
    // only a past session), so the fence is checked over every task.
    m.row(
        "work",
        "tree",
        |_, _| json!({ "action": "tree", "limit": 200, "filters": { "archived": true } }),
        move |_, who, a| {
            is_ok(who, a, "tree");
            let v: Value = serde_json::from_str(text(a)).unwrap();
            let keys: BTreeSet<String> = v["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|t| t["key"].as_str().map(String::from))
                .collect();
            let has = |k: &str| keys.contains(k);
            match who {
                w if w.is_unbound() => {
                    assert!(
                        has("AA-1") && has("BB-1") && has("BB-3") && has("LOC-1"),
                        "{who:?}: {keys:?}"
                    );
                    assert!(
                        text(a).contains("\"cross_org\":true"),
                        "the forced link is flagged"
                    );
                }
                Who::HostA => assert!(
                    has("AA-1") && !has("BB-1") && !has("BB-3") && !has("LOC-1"),
                    "own org, own host: {keys:?}"
                ),
                Who::HostB => {
                    assert!(has("BB-1") && has("BB-3") && !has("AA-1"), "{keys:?}");
                    // B's ticket still reaches host B; the A-host SESSION
                    // carrying it never does. Since multi-user M1 T6 a
                    // per-host token sees its own host's rows and nothing
                    // else, so this no longer turns on D7 — it subsumes it.
                    assert!(!text(a).contains("s-x"), "{a:?}");
                }
                Who::HostNone => assert!(has("LOC-1") && !has("AA-1") && !has("BB-1"), "{keys:?}"),
                Who::BoundA => {
                    assert!(has("AA-1") && has("LOC-1") && !has("BB-1"), "{keys:?}");
                }
                Who::BoundB => {
                    assert!(has("BB-1") && has("BB-3") && !has("AA-1"), "{keys:?}");
                    assert!(
                        !text(a).contains("s-x"),
                        "an A session never reaches a B-bound client"
                    );
                }
                _ => unreachable!(),
            }
        },
    )
    .await;
    m.row(
        "work",
        "task",
        |fx, _| json!({ "action": "task", "task_id": format!("item:{}", fx.item_b) }),
        move |_, who, a| match who {
            w if w.is_unbound() => {
                assert!(text(a).contains("s-x") && text(a).contains("s-b"), "{a:?}");
            }
            Who::HostB => {
                assert!(text(a).contains("s-b"), "{a:?}");
                // As in the `tree` row above: the task is B's, but s_x is a
                // session on host A, and a per-host token reaches no other
                // host's rows at all since M1 T6 — D7 or no D7.
                assert!(!text(a).contains("s-x"), "{a:?}");
            }
            Who::BoundB => assert!(text(a).contains("s-b") && !text(a).contains("s-x"), "{a:?}"),
            _ => is_code(who, a, "E_NOTFOUND", "another org's task"),
        },
    )
    .await;
    // No oracle: B's task answers org-A readers exactly as an unknown id.
    for who in [Who::HostA, Who::BoundA] {
        let hidden = call(
            &fx,
            who,
            "work",
            json!({ "action": "task", "task_id": format!("item:{}", fx.item_b) }),
        )
        .await;
        let unknown = call(
            &fx,
            who,
            "work",
            json!({ "action": "task", "task_id": "item:987654" }),
        )
        .await;
        same_as_unknown(&hidden, &unknown, &fx.item_b.to_string(), "987654");
    }
    m.row(
        "work",
        "session_tasks",
        |fx, _| json!({ "action": "session_tasks", "session_id": fx.s_x }),
        |fx, who, a| match who {
            w if w.is_unbound() => assert!(
                text(a).contains(&format!("\"link_id\":{}", relink(fx, fx.s_x, fx.item_b))),
                "{who:?}: {a:?}"
            ),
            // s_x is A's and on h-a: A's readers get the session without
            // B's task.
            Who::HostA | Who::BoundA => {
                is_ok(who, a, "own session");
                assert!(text(a).contains("\"links\":[]"), "{who:?}: {a:?}");
            }
            Who::BoundB => is_code(who, a, "E_NOTFOUND", "another org's session"),
            _ => is_code(who, a, "E_FORBIDDEN", "another host's session"),
        },
    )
    .await;
    m.row(
        "work",
        "review",
        |_, _| json!({ "action": "review" }),
        |fx, who, a| {
            is_ok(who, a, "review");
            let flagged =
                text(a).contains(&format!("\"link_id\":{}", relink(fx, fx.s_x, fx.item_b)));
            assert_eq!(flagged, who.is_unbound(), "{who:?}: {a:?}");
        },
    )
    .await;
    m.row(
        "work",
        "rules",
        |_, _| json!({ "action": "rules" }),
        |_, who, a| {
            is_ok(who, a, "rules");
            let t = text(a);
            let (sa, sb) = (t.contains("Alpha rule"), t.contains("Bravo rule"));
            match who {
                w if w.is_unbound() => assert!(sa && sb && t.contains("Alpha login"), "{t}"),
                // A host keeps no navigation.
                w if w.is_host() => assert_eq!(t, "[]", "{who:?}"),
                // A bound client: the rules that place a task it sees.
                Who::BoundA => assert!(sa && !sb && t.contains("Alpha login"), "{t}"),
                Who::BoundB => assert!(sb && !sa && !t.contains("Alpha login"), "{t}"),
                _ => unreachable!(),
            }
        },
    )
    .await;
    m.row(
        "work",
        "rule_preview",
        // An edit of B's rule, previewed: it moves B's tasks to "Pay".
        move |fx, _| {
            json!({ "action": "rule_preview",
                    "rule": { "id": bravo_rule, "name": "p",
                              "conditions": { "tracker_id": fx.tracker_b }, "group": "Pay" } })
        },
        |_, who, a| match who {
            w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "hosts do not preview rules"),
            Who::BoundA => {
                is_ok(who, a, "preview");
                assert!(
                    text(a).contains("\"total\":0"),
                    "nothing of B moves for A: {a:?}"
                );
            }
            _ => {
                is_ok(who, a, "preview");
                assert!(!text(a).contains("\"total\":0"), "{who:?}: {a:?}");
            }
        },
    )
    .await;
    m.row(
        "work",
        "views",
        |_, _| json!({ "action": "views" }),
        |_, who, a| {
            is_ok(who, a, "views");
            let names: Vec<String> = serde_json::from_str::<Vec<Value>>(text(a))
                .unwrap()
                .iter()
                .filter_map(|v| v["name"].as_str().map(String::from))
                .collect();
            match who {
                w if w.is_unbound() => assert_eq!(names.len(), 3, "{names:?}"),
                w if w.is_host() => assert!(names.is_empty(), "{who:?}: {names:?}"),
                // A bound client: its own org's views only (D35).
                Who::BoundA => assert_eq!(names, vec!["SECRET-A view".to_string()]),
                Who::BoundB => assert_eq!(names, vec!["SECRET-B view".to_string()]),
                _ => unreachable!(),
            }
        },
    )
    .await;
    m.row(
        "work",
        "org_impact",
        |fx, _| json!({ "action": "org_impact", "task_id": format!("item:{}", fx.item_b), "org_id": 0 }),
        |fx, who, a| match who {
            // Only a caller that may move an org reads the impact of a
            // move (D33): it names both orgs' hosts and bound clients.
            w if w.is_unbound() => {
                is_ok(who, a, "impact");
                let v: Value = serde_json::from_str(text(a)).unwrap();
                assert_eq!(v["reason"], "tracker_controlled", "{v}");
                assert_eq!(v["from_org"], fx.org_b, "{v}");
            }
            _ => is_code(who, a, "E_FORBIDDEN", "a scoped caller moves no org"),
        },
    )
    .await;
    // The refusal is the same for a task the caller sees and one it does
    // not: no oracle.
    for who in [Who::HostA, Who::BoundA, Who::BoundB] {
        let seen = call(
            &fx,
            who,
            "work",
            json!({ "action": "org_impact", "task_id": format!("item:{}", fx.item_a), "org_id": 0 }),
        )
        .await;
        let unseen = call(
            &fx,
            who,
            "work",
            json!({ "action": "org_impact", "task_id": "item:987654", "org_id": 0 }),
        )
        .await;
        assert_eq!(seen, unseen, "{who:?}");
    }
    // ── the Work view's writes (work graph M14.1c) ──────────────────────
    // Every write re-checks the caller's scope; a link, task or view out of
    // scope answers exactly as an unknown one, a version is compared only
    // after that, and readonly is refused by its mode.
    let primary_of = |fx: &Fx, sid: i64| -> i64 {
        fx.t.store
            .lock()
            .unwrap()
            .current_primary_link(sid)
            .unwrap()
            .unwrap_or(0)
    };
    let version_of = |fx: &Fx, link: i64| -> i64 {
        fx.t.store
            .lock()
            .unwrap()
            .work_link_version(link)
            .unwrap()
            .unwrap_or(0)
    };
    // The session a caller may act on but must not reach: s_x (A's session
    // carrying B's forced BB-1) for everyone but its own host's token and
    // the unrestricted callers.
    let s_x_refused = |who: Who, a: &Answer, ctx: &str| match who {
        Who::Master | Who::ClientFull => unreachable!(),
        // B's link on A's session: not A's to see; s_x not B's to reach.
        Who::HostA | Who::BoundA | Who::BoundB => is_code(who, a, "E_NOTFOUND", ctx),
        // Another host's session: the host fence.
        _ => is_code(who, a, "E_FORBIDDEN", ctx),
    };
    // Own primary, named as seen: an idempotent success for every writer.
    m.row(
        "work_link",
        "set_primary",
        move |fx, who| {
            let sid = own(fx, who);
            let p = primary_of(fx, sid);
            json!({ "action": "set_primary", "session_id": sid, "link_id": p, "expected_primary": p })
        },
        |_, who, a| {
            if !readonly_refused(who, a) {
                is_ok(who, a, "own primary");
            }
        },
    )
    .await;
    // A stale expectation is a conflict for every writer, naming only the
    // primary the caller sees.
    m.row(
        "work_link",
        "set_primary",
        move |fx, who| {
            let sid = own(fx, who);
            let p = primary_of(fx, sid);
            json!({ "action": "set_primary", "session_id": sid, "link_id": p,
                    "expected_primary": p + 1000 })
        },
        |_, who, a| {
            if !readonly_refused(who, a) {
                is_code(who, a, "E_CONFLICT", "stale primary");
                assert!(text(a).contains("primary_link_id"), "{who:?}: {a:?}");
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "set_primary",
        |fx, _| json!({ "action": "set_primary", "session_id": fx.s_x, "link_id": relink(fx, fx.s_x, fx.item_b) }),
        move |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            Who::Master | Who::ClientFull => is_ok(who, a, "the forced link is s_x's primary"),
            _ => s_x_refused(who, a, "set_primary on B's link / A's session"),
        },
    )
    .await;
    // Task → session P-2: a switch is the set_primary compare-and-set plus
    // the link's own target fence. The stale expectation keeps every row
    // from moving the fixture's links: the unrestricted callers reach the
    // compare-and-set and lose it, everyone else is refused before it.
    m.row(
        "work_link",
        "switch",
        |fx, _| {
            json!({ "action": "switch", "session_id": fx.s_x,
                    "link_id": relink(fx, fx.s_x, fx.item_b), "key": "ZZ-9",
                    "expected_primary": 987_654 })
        },
        move |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            Who::Master | Who::ClientFull => is_code(who, a, "E_CONFLICT", "stale primary"),
            _ => s_x_refused(who, a, "switch B's link on A's session"),
        },
    )
    .await;
    m.row(
        "work_link",
        "switch",
        move |fx, who| {
            let sid = own(fx, who);
            let p = primary_of(fx, sid);
            json!({ "action": "switch", "session_id": sid, "link_id": p, "key": "ZZ-9",
                    "expected_primary": p + 1000 })
        },
        |_, who, a| {
            if !readonly_refused(who, a) {
                is_code(who, a, "E_CONFLICT", "stale primary");
                assert!(text(a).contains("primary_link_id"), "{who:?}: {a:?}");
            }
        },
    )
    .await;
    // P-3: the live-elsewhere warning names only what the caller may see.
    // B's ticket is live on s_b (B's) and s_x (A's, forced). Out of scope it
    // answers as the unknown item it is; in B's scope s_x is another org's
    // session and is not counted, so the link goes ahead.
    m.row(
        "work_link",
        "link",
        move |fx, who| {
            json!({ "action": "link", "session_id": own(fx, who), "item_id": fx.item_b,
                    "primary": false, "ack_live": false })
        },
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            Who::Master | Who::ClientFull => {
                is_code(who, a, "E_EXISTS", "live elsewhere");
                assert!(text(a).contains("live_elsewhere"), "{who:?}: {a:?}");
            }
            Who::HostB | Who::BoundB => is_ok(who, a, "own link, nothing else in B"),
            _ => is_code(who, a, "E_NOTFOUND", "B's item out of scope"),
        },
    )
    .await;
    // Undo is for proposed links; the fixture's are made by hand.
    m.row(
        "work_link",
        "reconsider",
        move |fx, who| {
            let sid = own(fx, who);
            json!({ "action": "reconsider", "session_id": sid, "link_id": primary_of(fx, sid) })
        },
        |_, who, a| {
            if !readonly_refused(who, a) {
                is_code(
                    who,
                    a,
                    "E_INVALID_STATE",
                    "a link made by hand is removed, not undone",
                );
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "reconsider",
        |fx, _| json!({ "action": "reconsider", "session_id": fx.s_x, "link_id": relink(fx, fx.s_x, fx.item_b) }),
        move |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            Who::Master | Who::ClientFull => is_code(who, a, "E_INVALID_STATE", "by hand"),
            _ => s_x_refused(who, a, "reconsider B's link / A's session"),
        },
    )
    .await;
    // D32: the forced cross-org link is a review item until it is kept.
    // (Earlier rows unlink and re-make it: `relink` is its live id.)
    let cross_org_listed = |fx: &Fx| {
        let link = relink(fx, fx.s_x, fx.item_b);
        let r = crate::service::work::view::review(
            &fx.t.store,
            &crate::service::view_scope::ViewScope::internal(),
            None,
            None,
        )
        .unwrap();
        r.items
            .iter()
            .any(|i| i.kind == "cross_org" && i.link_id == link)
    };
    assert!(cross_org_listed(&fx), "D32: the forced link is reviewed");
    m.row(
        "work_link",
        "ack",
        move |fx, _| {
            let link = relink(fx, fx.s_x, fx.item_b);
            json!({ "action": "ack", "session_id": fx.s_x, "link_id": link,
                    "expected_version": version_of(fx, link) })
        },
        move |fx, who, a| match who {
            _ if readonly_refused(who, a) => {}
            Who::Master | Who::ClientFull => {
                is_ok(who, a, "keep the cross-org link");
                assert!(!cross_org_listed(fx), "kept: out of the inbox");
            }
            _ => s_x_refused(who, a, "ack B's link / A's session"),
        },
    )
    .await;
    {
        let s = fx.t.store.lock().unwrap();
        s.conn_for_test()
            .execute("UPDATE work_links SET review_ack_at = NULL", [])
            .unwrap();
    }
    // A batch always answers, item by item: the forced link as above, and
    // the caller's own primary confirmed again (allowed to every writer).
    m.row(
        "work_link",
        "decide_batch",
        move |fx, who| {
            let sid = own(fx, who);
            json!({ "action": "decide_batch", "decisions": [
                { "session_id": fx.s_x, "link_id": relink(fx, fx.s_x, fx.item_b), "decision": "ack" },
                { "session_id": sid, "link_id": primary_of(fx, sid), "decision": "confirm" },
            ] })
        },
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            is_ok(who, a, "a batch always answers");
            let v: Value = serde_json::from_str(text(a)).unwrap();
            let r = v["results"].as_array().unwrap();
            assert_eq!(r[0]["ok"], who.is_unbound(), "{who:?}: {v}");
            if !who.is_unbound() {
                let want = match who {
                    Who::HostA | Who::BoundA | Who::BoundB => "E_NOTFOUND",
                    _ => "E_FORBIDDEN",
                };
                assert_eq!(r[0]["code"], want, "{who:?}: {v}");
                assert!(r[0].get("version").is_none(), "{who:?}: {v}");
            }
            assert_eq!(r[1]["ok"], true, "{who:?}: one failing never stops the next: {v}");
        },
    )
    .await;
    {
        let s = fx.t.store.lock().unwrap();
        s.conn_for_test()
            .execute("UPDATE work_links SET review_ack_at = NULL", [])
            .unwrap();
    }
    // A hidden link's version is no oracle: with or without a (wrong)
    // expected_version, it answers as an unknown link.
    let link_x = relink(&fx, fx.s_x, fx.item_b);
    for who in [Who::HostA, Who::BoundA] {
        for action in [
            "confirm",
            "reject",
            "unlink",
            "set_primary",
            "reconsider",
            "ack",
        ] {
            let ask = |link: i64| {
                json!({ "action": action, "session_id": fx.s_x, "link_id": link,
                        "expected_version": 99 })
            };
            let hidden = call(&fx, who, "work_link", ask(link_x)).await;
            let unknown = call(&fx, who, "work_link", ask(999_999)).await;
            is_code(who, &hidden, "E_NOTFOUND", action);
            same_as_unknown(&hidden, &unknown, &link_x.to_string(), "999999");
            assert!(
                link_is_live(&fx, link_x),
                "{who:?} {action}: nothing changed"
            );
        }
    }
    // link / confirm { primary: false, expected_version }: the caller's own
    // work re-linked as a secondary keeps the primary; a stale version is a
    // conflict.
    let own_item = |fx: &Fx, who: Who| -> Value {
        match who {
            Who::HostB | Who::BoundB => json!({ "item_id": fx.item_b }),
            Who::HostNone => json!({ "key": "LOC-1" }),
            _ => json!({ "item_id": fx.item_a }),
        }
    };
    for stale in [false, true] {
        m.row(
            "work_link",
            "link",
            move |fx, who| {
                let sid = own(fx, who);
                let p = primary_of(fx, sid);
                let v = version_of(fx, p) + if stale { 5 } else { 0 };
                let mut args = json!({ "action": "link", "session_id": sid, "primary": false,
                                       "expected_version": v });
                args.as_object_mut()
                    .unwrap()
                    .extend(own_item(fx, who).as_object().unwrap().clone());
                args
            },
            move |fx, who, a| {
                if readonly_refused(who, a) {
                    return;
                }
                if stale {
                    return is_code(who, a, "E_CONFLICT", "a stale link version");
                }
                is_ok(who, a, "re-link own work as a secondary");
                let row: Value = serde_json::from_str(text(a)).unwrap();
                assert_eq!(
                    row["work"]["link_id"],
                    primary_of(fx, own(fx, who)),
                    "{who:?}"
                );
            },
        )
        .await;
        m.row(
            "work_link",
            "confirm",
            move |fx, who| {
                let sid = own(fx, who);
                let p = primary_of(fx, sid);
                let v = version_of(fx, p) + if stale { 5 } else { 0 };
                json!({ "action": "confirm", "session_id": sid, "link_id": p, "primary": false,
                        "expected_version": v })
            },
            move |_, who, a| {
                if readonly_refused(who, a) {
                    return;
                }
                if stale {
                    is_code(who, a, "E_CONFLICT", "a stale link version");
                } else {
                    is_ok(who, a, "confirm own primary as seen");
                }
            },
        )
        .await;
    }
    // Placement: never a host's; a bound client places only what it sees.
    let placement_of = |fx: &Fx, task: &str| -> i64 {
        fx.t.store
            .lock()
            .unwrap()
            .work_placement(task)
            .unwrap()
            .map_or(0, |p| p.version)
    };
    m.row(
        "work_link",
        "place",
        move |fx, who| {
            let task = format!("item:{}", fx.item_b);
            json!({ "action": "place", "task_id": task, "group": format!("Payroll {who:?}"),
                    "expected_version": placement_of(fx, &task) })
        },
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "hosts do not place"),
            Who::BoundA => is_code(who, a, "E_NOTFOUND", "B's task"),
            _ => {
                is_ok(who, a, "place");
                assert!(text(a).contains("\"source\":\"manual\""), "{who:?}: {a:?}");
                assert!(
                    text(a).contains(&format!("Payroll {who:?}")),
                    "{who:?}: {a:?}"
                );
            }
        },
    )
    .await;
    // A stale version: a conflict only for a caller who sees the task.
    m.row(
        "work_link",
        "place",
        move |fx, _| {
            let task = format!("item:{}", fx.item_b);
            json!({ "action": "place", "task_id": task, "group": "late",
                    "expected_version": placement_of(fx, &task) + 3 })
        },
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "hosts do not place"),
            Who::BoundA => is_code(who, a, "E_NOTFOUND", "B's task: no version oracle"),
            _ => is_code(who, a, "E_CONFLICT", "a stale placement"),
        },
    )
    .await;
    let hidden = call(
        &fx,
        Who::BoundA,
        "work_link",
        json!({ "action": "place", "task_id": format!("item:{}", fx.item_b), "group": "x", "expected_version": 1 }),
    )
    .await;
    let unknown = call(
        &fx,
        Who::BoundA,
        "work_link",
        json!({ "action": "place", "task_id": "item:999999", "group": "x", "expected_version": 1 }),
    )
    .await;
    same_as_unknown(&hidden, &unknown, &fx.item_b.to_string(), "999999");
    // Unassigned work (the bare key LOC-1): a bound client places it while
    // D31 is on (the matrix's default; off is below).
    m.row(
        "work_link",
        "place",
        move |fx, _| {
            json!({ "action": "place", "task_id": "ref:LOC-1", "group": "Local",
                    "expected_version": placement_of(fx, "ref:LOC-1") })
        },
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "hosts do not place"),
            _ => is_ok(who, a, "unassigned work, D31 on"),
        },
    )
    .await;
    // assign_org (D33): the master and a full unbound client, with a fresh
    // impact; never a host, never a bound client — refused before the task
    // is looked at, so no oracle either.
    m.row(
        "work_link",
        "assign_org",
        move |fx, _| {
            let task = format!("item:{local_a}");
            let cur = crate::service::work::view::task(
                &fx.t.store,
                &crate::service::view_scope::ViewScope::internal(),
                &task,
            )
            .unwrap()
            .task;
            let to = if cur.org_source == "item" { 0 } else { ORG_A };
            let imp = crate::service::work::structure::org_impact(
                &fx.t.store,
                &iso_view(&OrgScope::All),
                &task,
                Some(to),
            )
            .unwrap();
            json!({ "action": "assign_org", "task_id": task, "org_id": to,
                    "impact_token": imp.impact_token })
        },
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            Who::Master | Who::ClientFull => is_ok(who, a, "move a local task's org"),
            _ => is_code(who, a, "E_FORBIDDEN", "hosts and bound clients move no org"),
        },
    )
    .await;
    m.row(
        "work_link",
        "assign_org",
        |fx, _| json!({ "action": "assign_org", "task_id": format!("item:{}", fx.item_b), "org_id": 0, "impact_token": "stale" }),
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            Who::Master | Who::ClientFull => {
                is_code(who, a, "E_FORBIDDEN", "a tracker item's org is its tracker's")
            }
            _ => is_code(who, a, "E_FORBIDDEN", "hosts and bound clients move no org"),
        },
    )
    .await;
    m.row(
        "work_link",
        "assign_org",
        |_, _| json!({ "action": "assign_org", "task_id": "ref:LOC-1", "org_id": 1, "impact_token": "x" }),
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            Who::Master | Who::ClientFull => is_code(who, a, "E_INVALID", "a bare key"),
            _ => is_code(who, a, "E_FORBIDDEN", "hosts and bound clients move no org"),
        },
    )
    .await;
    {
        let s = fx.t.store.lock().unwrap();
        s.set_local_item_org(local_a, None).unwrap();
    }
    // Rules (D34): placement only, and the unrestricted callers' only.
    m.row(
        "work_link",
        "rule_save",
        |_, who| {
            json!({ "action": "rule_save",
                    "rule": { "name": format!("z {who:?}"), "conditions": { "key_prefix": "ZZ" }, "group": "Z" } })
        },
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            Who::Master | Who::ClientFull => is_ok(who, a, "rule"),
            _ => is_code(who, a, "E_FORBIDDEN", "rules reach every org"),
        },
    )
    .await;
    let rule_version = move |fx: &Fx| -> i64 {
        fx.t.store
            .lock()
            .unwrap()
            .work_rule(bravo_rule)
            .unwrap()
            .unwrap()
            .version
    };
    m.row(
        "work_link",
        "rule_save",
        move |fx, _| {
            json!({ "action": "rule_save",
                    "rule": { "id": bravo_rule, "name": "Bravo rule", "group": "SECRET-B group",
                              "conditions": { "tracker_id": fx.tracker_b },
                              "expected_version": rule_version(fx) + 1 } })
        },
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            Who::Master | Who::ClientFull => is_code(who, a, "E_CONFLICT", "a stale rule"),
            _ => is_code(who, a, "E_FORBIDDEN", "rules reach every org"),
        },
    )
    .await;
    let fresh_rule = std::cell::Cell::new(0_i64);
    let fresh_rule = &fresh_rule;
    m.row(
        "work_link",
        "rule_delete",
        move |fx, _| {
            let id = fx.t.store.lock().unwrap().seed_rule(
                "doomed",
                &crate::store::RuleConditions {
                    key_prefix: Some("DD".into()),
                    ..Default::default()
                },
                "D",
            );
            fresh_rule.set(id);
            json!({ "action": "rule_delete", "rule_id": id, "expected_version": 1 })
        },
        move |fx, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            let gone =
                fx.t.store
                    .lock()
                    .unwrap()
                    .work_rule(fresh_rule.get())
                    .unwrap()
                    .is_none();
            match who {
                Who::Master | Who::ClientFull => {
                    is_ok(who, a, "delete");
                    assert!(gone, "{who:?}");
                }
                _ => {
                    is_code(who, a, "E_FORBIDDEN", "rules reach every org");
                    assert!(!gone, "{who:?}: a refusal deletes nothing");
                }
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "rule_delete",
        |_, _| json!({ "action": "rule_delete", "rule_id": 987654 }),
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            Who::Master | Who::ClientFull => is_code(who, a, "E_NOTFOUND", "no such rule"),
            _ => is_code(who, a, "E_FORBIDDEN", "rules reach every org"),
        },
    )
    .await;
    // Views (D35): shared on the hub, a bound client's its org's.
    m.row(
        "work_link",
        "view_save",
        |_, who| {
            json!({ "action": "view_save",
                    "view": { "name": format!("open {who:?}"), "filters": { "status": "open" } } })
        },
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "hosts keep no views"),
            _ => is_ok(who, a, "own view"),
        },
    )
    .await;
    let listed = call(&fx, Who::BoundA, "work", json!({ "action": "views" })).await;
    assert!(
        text(&listed).contains("open BoundA")
            && !text(&listed).contains("open BoundB")
            && !text(&listed).contains("open Master"),
        "{listed:?}"
    );
    // Another org's view: replaced only by its org and the unrestricted.
    let view_b: i64 = serde_json::from_str::<Vec<Value>>(text(
        &call(&fx, Who::BoundB, "work", json!({ "action": "views" })).await,
    ))
    .unwrap()
    .iter()
    .find(|v| v["name"] == "SECRET-B view")
    .and_then(|v| v["id"].as_i64())
    .unwrap();
    let unknown_view = &call(
        &fx,
        Who::BoundA,
        "work_link",
        json!({ "action": "view_save",
                "view": { "id": 999_999, "name": "SECRET-B view", "filters": { "org": ORG_B } } }),
    )
    .await;
    m.row(
        "work_link",
        "view_save",
        move |_, _| {
            json!({ "action": "view_save",
                    "view": { "id": view_b, "name": "SECRET-B view", "filters": { "org": ORG_B } } })
        },
        move |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "hosts keep no views"),
            Who::BoundA => {
                is_code(who, a, "E_NOTFOUND", "another org's view");
                same_as_unknown(a, unknown_view, &view_b.to_string(), "999999");
            }
            _ => {
                is_ok(who, a, "the org's own view, or the master's reach");
                let v: Value = serde_json::from_str(text(a)).unwrap();
                assert_eq!(v["owner_org"], ORG_B, "{who:?}: the owner stays");
            }
        },
    )
    .await;
    // A bound client's view names only what it sees.
    m.row(
        "work_link",
        "view_save",
        |fx, who| {
            json!({ "action": "view_save",
                    "view": { "name": format!("tracker {who:?}"), "filters": { "tracker": fx.tracker_b } } })
        },
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "hosts keep no views"),
            Who::BoundA => is_code(who, a, "E_NOTFOUND", "B's tracker"),
            _ => is_ok(who, a, "a tracker it sees"),
        },
    )
    .await;
    let doomed_view = std::cell::Cell::new(0_i64);
    let doomed_view = &doomed_view;
    m.row(
        "work_link",
        "view_delete",
        move |fx, who| {
            let id = fx.t.store.lock().unwrap().seed_view(
                &format!("doomed {who:?}"),
                &json!({}),
                Some(ORG_A),
            );
            doomed_view.set(id);
            json!({ "action": "view_delete", "view_id": id, "expected_version": 1 })
        },
        move |fx, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            let gone =
                fx.t.store
                    .lock()
                    .unwrap()
                    .work_view(doomed_view.get())
                    .unwrap()
                    .is_none();
            match who {
                w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "hosts keep no views"),
                Who::BoundB => is_code(who, a, "E_NOTFOUND", "A's view"),
                _ => {
                    is_ok(who, a, "delete");
                    assert!(gone, "{who:?}");
                    return;
                }
            }
            assert!(!gone, "{who:?}: a refusal deletes nothing");
        },
    )
    .await;
    m.row(
        "work_link",
        "view_delete",
        |_, _| json!({ "action": "view_delete", "view_id": 987654 }),
        |_, who, a| match who {
            _ if readonly_refused(who, a) => {}
            w if w.is_host() => is_code(who, a, "E_FORBIDDEN", "hosts keep no views"),
            _ => is_code(who, a, "E_NOTFOUND", "no such view"),
        },
    )
    .await;
    // D31's switch is org administration: the master's, and nobody else's.
    m.row(
        "work_admin",
        "update_org",
        |_, _| json!({ "action": "update_org", "org_id": ORG_A, "bound_sees_unassigned": true }),
        |fx, who, a| match who {
            Who::Master => {
                is_ok(who, a, "D31");
                let v: Value = serde_json::from_str(text(a)).unwrap();
                assert_eq!(v["bound_sees_unassigned"], true, "{v}");
                let _ = fx;
            }
            _ => is_code(who, a, "E_FORBIDDEN", "work_admin is master-only"),
        },
    )
    .await;
    {
        let s = fx.t.store.lock().unwrap();
        s.conn_for_test()
            .execute_batch(
                "DELETE FROM work_placements; DELETE FROM work_rules; DELETE FROM work_views; \
                 UPDATE work_links SET review_ack_at = NULL;",
            )
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

/// D31 (work graph M14.1b), its second value: with an org's
/// `bound_sees_unassigned` off — set through `work_admin`'s org edit, as the
/// operator does — its bound clients read only rows assigned to their org:
/// no unassigned session (s_n on the no-org host h-n) and no unassigned
/// work (its bare key LOC-1), through any of the Work view's reads, nor
/// the session list. The master and the hosts read as before: D31 is a
/// bound client's switch only. The matrix above runs with the default (on).
#[tokio::test]
async fn the_work_view_reads_follow_d31_off() {
    let fx = fixture(false);
    for org in [ORG_A, ORG_B] {
        let a = call(
            &fx,
            Who::Master,
            "work_admin",
            json!({ "action": "update_org", "org_id": org, "bound_sees_unassigned": false }),
        )
        .await;
        is_ok(Who::Master, &a, "D31 off");
        assert!(
            text(&a).contains("\"bound_sees_unassigned\":false"),
            "{a:?}"
        );
    }
    // What an unassigned row would show: its key and its session's name.
    const UNASSIGNED: &[&str] = &["LOC-1", "s-n"];
    let reads = |fx: &Fx| -> Vec<(&'static str, Value)> {
        vec![
            ("tree", json!({ "action": "tree", "limit": 200 })),
            ("task", json!({ "action": "task", "task_id": "ref:LOC-1" })),
            (
                "session_tasks",
                json!({ "action": "session_tasks", "session_id": fx.s_n }),
            ),
            ("review", json!({ "action": "review" })),
            ("rules", json!({ "action": "rules" })),
            (
                "rule_preview",
                json!({ "action": "rule_preview",
                        "rule": { "name": "loc", "conditions": { "key_prefix": "LOC" }, "group": "Loc" } }),
            ),
            ("views", json!({ "action": "views" })),
            (
                "org_impact",
                json!({ "action": "org_impact", "task_id": "ref:LOC-1", "org_id": 0 }),
            ),
        ]
    };
    for who in [
        Who::Master,
        Who::HostA,
        Who::HostNone,
        Who::BoundA,
        Who::BoundB,
    ] {
        for (action, args) in reads(&fx) {
            let a = call(&fx, who, "work", args).await;
            let t = text(&a);
            for m in who.forbidden_markers() {
                assert!(
                    !t.contains(m),
                    "LEAK: {who:?} read {m:?} through {action}: {t}"
                );
            }
            if who.is_bound() {
                // The asked-for key may come back in its own refusal.
                for m in UNASSIGNED
                    .iter()
                    .filter(|m| !(action == "task" && **m == "LOC-1"))
                {
                    assert!(
                        !t.contains(m),
                        "D31 off: {who:?} read the unassigned {m:?} through {action}: {t}"
                    );
                }
            }
            match (who, action) {
                (Who::Master, "tree") | (Who::HostNone, "tree") => {
                    assert!(t.contains("LOC-1"), "unchanged for {who:?}: {t}")
                }
                (Who::Master, "task" | "session_tasks") => is_ok(who, &a, action),
                (Who::HostNone, "task" | "session_tasks") => is_ok(who, &a, "its own host"),
                (Who::BoundA | Who::BoundB, "task" | "session_tasks") => {
                    is_code(who, &a, "E_NOTFOUND", "unassigned, D31 off")
                }
                (Who::BoundA, "tree") => assert!(t.contains("AA-1"), "its own org: {t}"),
                (Who::BoundB, "tree") => assert!(t.contains("BB-1"), "its own org: {t}"),
                (Who::BoundA | Who::BoundB, "rule_preview") => {
                    assert!(t.contains("\"total\":0"), "{who:?}: {t}")
                }
                (Who::Master, "rule_preview") => assert!(t.contains("LOC-1"), "{t}"),
                (w, "org_impact") if !w.is_unbound() => {
                    is_code(who, &a, "E_FORBIDDEN", "a scoped caller moves no org")
                }
                _ => {}
            }
        }
        // No oracle: the hidden bare key reads as a key nobody linked.
        if who.is_bound() {
            let hidden = call(
                &fx,
                who,
                "work",
                json!({ "action": "task", "task_id": "ref:LOC-1" }),
            )
            .await;
            let unknown = call(
                &fx,
                who,
                "work",
                json!({ "action": "task", "task_id": "ref:NOPE-1" }),
            )
            .await;
            same_as_unknown(&hidden, &unknown, "LOC-1", "NOPE-1");
        }
        // The session list follows the same switch.
        let a = call(&fx, who, "list_sessions", json!({})).await;
        if who.is_bound() {
            assert!(!text(&a).contains("s-n"), "{who:?}: {a:?}");
        }
    }
    // Nor does it start a session it could not see: an unassigned project
    // on the no-org host is refused while the switch is off.
    {
        let s = fx.t.store.lock().unwrap();
        let loose = s.upsert_project("loose", "x", "/src/loose").unwrap();
        let err = super::support::require_bound_client_may_create(
            &s,
            &Who::BoundA.caller(),
            "h-n",
            loose,
        )
        .unwrap_err();
        assert!(err.message.starts_with("E_FORBIDDEN"), "{err:?}");
        assert!(super::support::require_bound_client_may_create(
            &s,
            &Who::BoundA.caller(),
            "h-a",
            fx.pid_acme
        )
        .is_ok());
    }
    // Back on: A's bound client reads the unassigned rows again, from its
    // next call on (the scope is read with each call).
    call(
        &fx,
        Who::Master,
        "work_admin",
        json!({ "action": "update_org", "org_id": ORG_A, "bound_sees_unassigned": true }),
    )
    .await
    .unwrap();
    let a = call(
        &fx,
        Who::BoundA,
        "work",
        json!({ "action": "tree", "limit": 200 }),
    )
    .await;
    assert!(
        text(&a).contains("LOC-1") && text(&a).contains("s-n"),
        "{a:?}"
    );
    let a = call(
        &fx,
        Who::BoundB,
        "work",
        json!({ "action": "tree", "limit": 200 }),
    )
    .await;
    assert!(
        !text(&a).contains("LOC-1"),
        "B's switch is still off: {a:?}"
    );
}

/// Every Routed work command of the desktop maps to an action the matrix
/// covers (src-tauri's routing tests hold `ROUTED_WORK_COMMANDS` to its
/// verdict table).
/// D31's second value for the writes (work graph M14.1c): with an org's
/// `bound_sees_unassigned` off, its bound clients cannot write to
/// unassigned work or sessions either — placing the bare key LOC-1,
/// deciding on s_n's link, alone or in a batch, or naming the unassigned
/// work in a view all answer as unknown. Their own org's writes, the
/// master's and the hosts' are unchanged. The matrix above covers D31 on.
#[tokio::test]
async fn the_work_view_writes_follow_d31_off() {
    let fx = fixture(false);
    for org in [ORG_A, ORG_B] {
        let a = call(
            &fx,
            Who::Master,
            "work_admin",
            json!({ "action": "update_org", "org_id": org, "bound_sees_unassigned": false }),
        )
        .await;
        is_ok(Who::Master, &a, "D31 off");
    }
    let s_n_link =
        fx.t.store
            .lock()
            .unwrap()
            .current_primary_link(fx.s_n)
            .unwrap()
            .unwrap();
    let writes = |fx: &Fx| -> Vec<(&'static str, Value)> {
        vec![
            (
                "place",
                json!({ "action": "place", "task_id": "ref:LOC-1", "group": "x", "expected_version": 0 }),
            ),
            (
                "set_primary",
                json!({ "action": "set_primary", "session_id": fx.s_n, "link_id": s_n_link }),
            ),
            (
                "ack",
                json!({ "action": "ack", "session_id": fx.s_n, "link_id": s_n_link }),
            ),
            (
                "reconsider",
                json!({ "action": "reconsider", "session_id": fx.s_n, "link_id": s_n_link }),
            ),
            (
                "unlink",
                json!({ "action": "unlink", "session_id": fx.s_n, "link_id": s_n_link,
                        "expected_version": 1 }),
            ),
        ]
    };
    for who in [Who::BoundA, Who::BoundB] {
        for (action, args) in writes(&fx) {
            let a = call(&fx, who, "work_link", args).await;
            is_code(who, &a, "E_NOTFOUND", action);
        }
        let batch = call(
            &fx,
            who,
            "work_link",
            json!({ "action": "decide_batch", "decisions": [
                { "session_id": fx.s_n, "link_id": s_n_link, "decision": "confirm" },
            ] }),
        )
        .await;
        is_ok(who, &batch, "a batch always answers");
        let v: Value = serde_json::from_str(text(&batch)).unwrap();
        assert_eq!(v["results"][0]["code"], "E_NOTFOUND", "{who:?}: {v}");
        // Its own org's work is still its to place.
        let own = if who == Who::BoundA {
            fx.item_a
        } else {
            fx.item_b
        };
        let placed = call(
            &fx,
            who,
            "work_link",
            json!({ "action": "place", "task_id": format!("item:{own}"), "group": format!("{who:?}"),
                    "expected_version": 0 }),
        )
        .await;
        is_ok(who, &placed, "own org's task");
    }
    assert!(link_is_live(&fx, s_n_link), "a refusal changes nothing");
    // The master, the no-org host and an unbound client: as before.
    for who in [Who::Master, Who::ClientFull, Who::HostNone] {
        let p = call(
            &fx,
            who,
            "work_link",
            json!({ "action": "set_primary", "session_id": fx.s_n, "link_id": s_n_link,
                    "expected_primary": s_n_link }),
        )
        .await;
        is_ok(who, &p, "D31 is a bound client's switch only");
    }
    let placed = call(
        &fx,
        Who::Master,
        "work_link",
        json!({ "action": "place", "task_id": "ref:LOC-1", "group": "x", "expected_version": 0 }),
    )
    .await;
    is_ok(Who::Master, &placed, "unassigned work, the master");
}

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
/// Work graph M12.4: `fleet_health.trackers` is org-scoped for a per-host
/// token — its own org's trackers only (a host in no org: unassigned ones
/// only), its own host's backlog — and every caller gets the errors fenced.
#[tokio::test]
async fn fleet_healths_tracker_roll_up_is_fenced_by_org() {
    let fx = fixture(false);
    {
        let s = fx.t.store.lock().unwrap();
        s.set_tracker_state(fx.tracker_a, "auth_failed", Some("SECRET-A token expired"))
            .unwrap();
        s.set_tracker_state(fx.tracker_b, "auth_failed", Some("SECRET-B token expired"))
            .unwrap();
    }
    let rollup = |a: &Answer| -> Vec<i64> {
        let v: Value = serde_json::from_str(text(a)).unwrap();
        v["trackers"]["trackers"]
            .as_array()
            .map(|ts| {
                ts.iter()
                    .map(|t| t["tracker_id"].as_i64().unwrap())
                    .collect()
            })
            .unwrap_or_default()
    };
    for who in EVERYONE {
        let a = call(&fx, *who, "fleet_health", json!({})).await;
        assert_eq!(code(&a), "OK", "{who:?}: {a:?}");
        for m in who.forbidden_markers() {
            assert!(!text(&a).contains(m), "{who:?} read {m}: {}", text(&a));
        }
        let want = match who {
            Who::HostA | Who::BoundA => vec![fx.tracker_a],
            Who::HostB | Who::BoundB => vec![fx.tracker_b],
            Who::HostNone => vec![],
            _ => vec![fx.tracker_a, fx.tracker_b],
        };
        assert_eq!(rollup(&a), want, "{who:?}");
        if !want.is_empty() {
            let v: Value = serde_json::from_str(text(&a)).unwrap();
            let e = v["trackers"]["trackers"][0]["last_error"].as_str().unwrap();
            assert!(
                e.starts_with(&crate::mcp::guard::untrusted_marker(
                    crate::service::health::TRACKER_ERROR_FROM
                )),
                "{who:?}: {e}"
            );
            assert_eq!(v["trackers"]["failing"], json!(want.len()), "{who:?}");
        }
        if who.is_host() {
            let other = if *who == Who::HostA {
                "B Jira"
            } else {
                "A Jira"
            };
            assert!(!text(&a).contains(other), "{who:?}: {}", text(&a));
        }
        // Work graph M14: a bound client is not told the other org's
        // tracker, nor its host's spend.
        if who.is_bound() {
            let (other, other_host) = if *who == Who::BoundA {
                ("B Jira", "h-b")
            } else {
                ("A Jira", "h-a")
            };
            assert!(!text(&a).contains(other), "{who:?}: {}", text(&a));
            let v: Value = serde_json::from_str(text(&a)).unwrap();
            let hosts: Vec<&str> = v["usage_by_host"]
                .as_object()
                .map(|m| m.keys().map(String::as_str).collect())
                .unwrap_or_default();
            assert!(!hosts.contains(&other_host), "{who:?}: {hosts:?}");
        }
    }
}

/// The decision envelope's health is the hub's: the master and an unbound
/// paired client read it, a per-host token and an org-bound client do not.
#[tokio::test]
async fn fleet_healths_decide_block_is_the_hubs_alone() {
    let fx = fixture(false);
    {
        let s = fx.t.store.lock().unwrap();
        crate::service::settings::set(&s, crate::service::settings::DECIDE_JEV_ENABLED, "true")
            .unwrap();
        crate::service::settings::set(
            &s,
            crate::service::settings::DECIDE_JEV_STATUS_MAP,
            "shadow",
        )
        .unwrap();
    }
    for who in EVERYONE {
        let a = call(&fx, *who, "fleet_health", json!({})).await;
        assert_eq!(code(&a), "OK", "{who:?}: {a:?}");
        let v: Value = serde_json::from_str(text(&a)).unwrap();
        let sees = !(who.is_host() || who.is_bound());
        assert_eq!(v.get("decide").is_some(), sees, "{who:?}: {v}");
        if sees {
            assert_eq!(
                v["decide"]["modes"]["status_map"],
                json!("shadow"),
                "{who:?}"
            );
        }
    }
}

/// Spend on A's, B's and the unassigned host's sessions: 100, 20 000 and
/// 3 000 micro-USD, all today.
fn seed_usage(fx: &Fx) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    {
        let s = fx.t.store.lock().unwrap();
        for (id, host, cost) in [
            (fx.s_a, "h-a", 100),
            (fx.s_b, "h-b", 20_000),
            (fx.s_n, "h-n", 3_000),
        ] {
            s.apply_usage(
                id,
                host,
                &crate::store::UsageDelta {
                    reset: false,
                    totals: crate::store::UsageTotals {
                        input_tokens: cost,
                        cost_micros: cost,
                        ..Default::default()
                    },
                    model: None,
                    offset: 1,
                    source: "x.jsonl".into(),
                    last_msg_id: None,
                    last_msg_usage: None,
                    now,
                    by_day: Vec::new(),
                    backfill_until: None,
                },
            )
            .unwrap();
        }
    }
}

/// Work graph M14: every roll-up in `fleet_health` that sums across hosts
/// (spend by host and by day, host and session counts) is taken over the
/// hosts an org-bound client sees — its org's, and unassigned ones only
/// while D31 is on — so B's spend never reaches A's client, not even as a
/// fleet-wide total. Everyone else reads as before.
#[tokio::test]
async fn fleet_healths_totals_are_fenced_for_an_org_bound_client() {
    let fx = fixture(false);
    seed_usage(&fx);
    // (daily cost, usage_by_host keys, hosts_total, sessions_total,
    // hosts[] aliases)
    let read = |a: &Answer| -> (i64, Vec<String>, i64, i64, Vec<String>) {
        let v: Value = serde_json::from_str(text(a)).unwrap();
        let day: i64 = v["usage_by_day"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["cost_micros"].as_i64().unwrap())
            .sum();
        let hosts = v["usage_by_host"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let mut rows: Vec<String> = v["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["alias"].as_str().unwrap().to_string())
            .collect();
        rows.sort();
        (
            day,
            hosts,
            v["hosts_total"].as_i64().unwrap(),
            v["sessions_total"].as_i64().unwrap(),
            rows,
        )
    };
    let names = |hs: &[&str]| hs.iter().map(|h| h.to_string()).collect::<Vec<_>>();
    let all = names(&["h-a", "h-b", "h-n"]);
    let everything = (23_100, all.clone(), 3, 4, all);
    let (an, bn) = (names(&["h-a", "h-n"]), names(&["h-b", "h-n"]));
    let (ha, hb, hn) = (names(&["h-a"]), names(&["h-b"]), names(&["h-n"]));
    for who in EVERYONE {
        let a = call(&fx, *who, "fleet_health", json!({})).await;
        assert_eq!(code(&a), "OK", "{who:?}: {a:?}");
        let want = match who {
            // D31 on (the default): the org's hosts and h-n. s_x is A's
            // too; B's s_b and its 20 000 are nowhere.
            // hosts[] (disk, load, versions) is narrowed the same way.
            Who::BoundA => (3_100, an.clone(), 2, 3, an.clone()),
            Who::BoundB => (23_000, bn.clone(), 2, 2, bn.clone()),
            // As before: its own host's spend, fleet-wide counts; hosts[]
            // is its own host's row only.
            Who::HostA => (100, ha.clone(), 3, 4, ha.clone()),
            Who::HostB => (20_000, hb.clone(), 3, 4, hb.clone()),
            Who::HostNone => (3_000, hn.clone(), 3, 4, hn.clone()),
            _ => everything.clone(),
        };
        assert_eq!(read(&a), want, "{who:?}");
    }
    // D31 off: the unassigned host leaves a bound client's totals too.
    for org in [ORG_A, ORG_B] {
        let a = call(
            &fx,
            Who::Master,
            "work_admin",
            json!({ "action": "update_org", "org_id": org, "bound_sees_unassigned": false }),
        )
        .await;
        is_ok(Who::Master, &a, "D31 off");
    }
    for (who, want) in [
        (Who::BoundA, (100, ha.clone(), 1, 2, ha.clone())),
        (Who::BoundB, (20_000, hb.clone(), 1, 1, hb.clone())),
        (Who::Master, everything.clone()),
        (Who::ClientReadonly, everything.clone()),
    ] {
        let a = call(&fx, who, "fleet_health", json!({})).await;
        assert_eq!(read(&a), want, "{who:?} with D31 off");
    }
}

/// `usage_report` is fenced like `fleet_health`: a client bound to an org
/// sees its org's sessions and hosts (and, D31 on, the unassigned host's),
/// never another org's spend; a host outside that answers as unknown.
#[tokio::test]
async fn usage_report_is_fenced_for_an_org_bound_client() {
    let fx = fixture(false);
    seed_usage(&fx);
    // (total cost, by_host keys, session ids)
    let read = |a: &Answer| -> (i64, Vec<String>, Vec<i64>) {
        let v: Value = serde_json::from_str(text(a)).unwrap();
        let hosts = v["by_host"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let mut ids: Vec<i64> = v["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["session_id"].as_i64().unwrap())
            .collect();
        ids.sort_unstable();
        (v["total"]["cost_micros"].as_i64().unwrap(), hosts, ids)
    };
    let names = |hs: &[&str]| hs.iter().map(|h| h.to_string()).collect::<Vec<_>>();
    let mut all = vec![fx.s_a, fx.s_b, fx.s_n];
    all.sort_unstable();
    let a = call(&fx, Who::Master, "usage_report", json!({})).await;
    assert_eq!(read(&a), (23_100, names(&["h-a", "h-b", "h-n"]), all));
    let a = call(&fx, Who::BoundA, "usage_report", json!({})).await;
    let mut seen = vec![fx.s_a, fx.s_n];
    seen.sort_unstable();
    assert_eq!(read(&a), (3_100, names(&["h-a", "h-n"]), seen));
    let a = call(
        &fx,
        Who::BoundA,
        "usage_report",
        json!({ "host_alias": "h-b" }),
    )
    .await;
    assert_eq!(code(&a), "E_NOTFOUND", "{a:?}");
    let a = call(
        &fx,
        Who::BoundB,
        "usage_report",
        json!({ "host_alias": "h-b" }),
    )
    .await;
    assert_eq!(read(&a), (20_000, names(&["h-b"]), vec![fx.s_b]));
}

/// Work graph M13.1: a tracker skipping items reads through the same org
/// fence — a per-host token sees only its own org's trackers, their counts
/// and the skipped item's reason, never another org's; and the reason is
/// `items_skipped` for every caller, never a credential one.
#[test]
fn a_skipping_trackers_health_is_fenced_by_org_too() {
    use crate::service::health::{trackers_from_store, TRACKER_REASON_ITEMS_SKIPPED};
    use crate::service::trackers::sync::SyncMetrics;
    let fx = fixture(false);
    let (ta, tb) = (fx.tracker_a, fx.tracker_b);
    {
        let s = fx.t.store.lock().unwrap();
        s.set_tracker_state(ta, "ok", None).unwrap();
        s.set_tracker_state(tb, "ok", None).unwrap();
    }
    let metrics = move |ids: &[i64]| -> Vec<SyncMetrics> {
        ids.iter()
            .map(|&id| SyncMetrics {
                tracker_id: id,
                last_pass_at: Some(1),
                items_failed: 1,
                consecutive_partial: 1,
                last_item_error: Some(
                    if id == ta {
                        "SECRET-A poison"
                    } else {
                        "SECRET-B poison"
                    }
                    .into(),
                ),
                ..Default::default()
            })
            .collect()
    };
    for who in EVERYONE {
        let s = fx.t.store.lock().unwrap();
        // The scope `fleet_health` fences a caller's roll-up by.
        let scope = who.caller().org_scope(&s).unwrap();
        let h = trackers_from_store(&s, &scope, &metrics, 1);
        let body = serde_json::to_string(&h).unwrap();
        for m in who.forbidden_markers() {
            assert!(!body.contains(m), "{who:?} read {m}: {body}");
        }
        let want = match who {
            Who::HostA | Who::BoundA => vec![ta],
            Who::HostB | Who::BoundB => vec![tb],
            Who::HostNone => vec![],
            _ => vec![ta, tb],
        };
        let got: Vec<i64> = h.trackers.iter().map(|t| t.tracker_id).collect();
        assert_eq!(got, want, "{who:?}");
        for t in &h.trackers {
            assert_eq!(t.health, "degraded", "{who:?}");
            assert_eq!(t.reason, TRACKER_REASON_ITEMS_SKIPPED, "{who:?}");
            assert_eq!((t.items_failed, t.consecutive_partial), (1, 1));
        }
        assert_eq!(h.degraded as usize, want.len(), "{who:?}");
    }
}

#[tokio::test]
async fn a_per_host_token_still_cannot_read_another_hosts_tickets_in_its_org() {
    let fx = fixture(false);
    {
        let s = fx.t.store.lock().unwrap();
        s.upsert_host("h-a2").unwrap();
        s.set_host_org("h-a2", s.host_org("h-a").unwrap()).unwrap();
    }
    let a2 = Caller {
        api: None,
        host_alias: Some("h-a2".into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
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
    for who in [Who::ClientReadonly, Who::HostA, Who::HostNone] {
        assert!(
            enforce_admin(&who.caller(), "work_admin").is_err()
                || !crate::mcp::guard::access_allows(&who.caller(), "work_admin")
                || enforce_mode(&who.caller(), "work_admin").is_err(),
            "{who:?}"
        );
    }
    assert!(enforce_admin(&Caller::master(), "work_admin").is_ok());
    // Contract 13: the owner's own full device passes the gate for its
    // trackers; `work_admin` itself refuses it every org action (and every
    // action until it is trusted), which the matrix's work_admin rows prove
    // for the untrusted fixture.
    assert!(present::visible_to(&Who::ClientFull.caller(), "work_admin"));
    for name in crate::service::trackers::admin::AdminAction::NAMES {
        let action = crate::service::trackers::admin::AdminAction::parse(name).unwrap();
        if name.contains("org") {
            assert!(!action.is_tracker_action(), "{name}");
        }
    }
    assert!(!present::visible_to(&Who::HostA.caller(), "work_admin"));
    let _ = OrgScope::All;
}

/// The classification nudge (work graph M4.6) is read by the Claude on the
/// session's host, so it offers only what that host may read. B's ticket is
/// "mine" to the tracker account and even force-linked on h-a — the host
/// fence alone would let it through — yet h-a's note never names it; h-b's
/// does.
#[test]
fn the_classification_nudge_offers_only_the_hosts_own_tickets() {
    let fx = fixture(false);
    let s = fx.t.store.lock().unwrap();
    let me = crate::store::TrackerConfig {
        account_id: Some("me".into()),
        ..Default::default()
    };
    for t in [fx.tracker_a, fx.tracker_b] {
        s.set_tracker_probe(t, None, &me).unwrap();
    }
    let mine = |tracker: i64, key: &str, title: &str| {
        s.upsert_tracker_item(
            tracker,
            &TrackerItemWrite {
                external_id: format!("mine-{key}"),
                key: Some(key.into()),
                title: title.into(),
                status_name: "To Do".into(),
                status_category: "todo".into(),
                assignee_id: Some("me".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    };
    let aa9 = mine(fx.tracker_a, "AA-9", "Alpha nine");
    let bb9 = mine(fx.tracker_b, "BB-9", "Bravo nine");
    s.link_session_work(fx.s_a, WorkTarget::Item(aa9), "manual")
        .unwrap();
    s.link_session_work(fx.s_b, WorkTarget::Item(bb9), "manual")
        .unwrap();
    s.link_session_work(fx.s_x, WorkTarget::Item(bb9), "manual")
        .unwrap();
    crate::service::settings::set(&s, crate::service::settings::WORK_CLASSIFY_NUDGE, "true")
        .unwrap();
    let fresh = |host: &str, conv: &str| {
        let id = s
            .upsert_session(conv, host, None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, conv).unwrap();
        s.rebind_conversation(id, conv, crate::store::StartSource::Fleet, None, None)
            .unwrap();
        for _ in 0..crate::service::work::nudge::NUDGE_AFTER_TURNS {
            s.conversation_bump_turns(id, conv).unwrap();
        }
        let row = s.get_session_by_id(id).unwrap().unwrap();
        crate::service::work::nudge::classify_nudge(&s, &row, conv, 0).unwrap()
    };
    let a = fresh("h-a", "conv-nudge-a").expect("A's own ticket is offered");
    assert!(a.contains("AA-9"), "{a}");
    for m in B_MARKERS.iter().chain(["BB-9", "Bravo nine"].iter()) {
        assert!(!a.contains(m), "LEAK in h-a's nudge: {m:?}: {a}");
    }
    let b = fresh("h-b", "conv-nudge-b").expect("B's own ticket is offered on h-b");
    assert!(b.contains("BB-9"), "{b}");
    assert!(!b.contains("AA-9"), "{b}");
}

/// Work graph M14.1d: `work:changed` is emitted — ids only — on a
/// placement, a rule, a saved view and a local task's org change, and on
/// `/events` it reaches only a caller that reads every org. A per-host
/// token and an org-bound client never receive it (kind `work`): a frame
/// naming a task, rule or view carries no session to fence it by, so they
/// re-read what they may see through `work { … }` on their own
/// `session:*` frames instead.
#[test]
fn work_changed_carries_ids_only_and_reaches_only_unbound_callers() {
    use crate::events::BroadcastEventBus;
    use crate::mcp::events_route::{fence_frame, fence_host_bound, matches};
    use crate::service::work::structure::{self as st, RuleInput, ViewInput};
    use crate::store::RuleConditions;

    let bus = std::sync::Arc::new(BroadcastEventBus::default());
    // Subscribed first: the bus renders nothing nobody listens for.
    let mut rx = bus.subscribe();
    let store = std::sync::Mutex::new(Store::open_with_bus_in_memory(bus.clone()).unwrap());
    let local = {
        let s = store.lock().unwrap();
        for h in ["h-a", "h-b", "h-n"] {
            s.upsert_host(h).unwrap();
        }
        let a = s.add_org("Alpha", None, false).unwrap();
        let b = s.add_org("Bravo", None, false).unwrap();
        assert_eq!((a.id, b.id), (ORG_A, ORG_B), "the callers' org ids");
        s.set_host_org("h-a", Some(ORG_A)).unwrap();
        s.set_host_org("h-b", Some(ORG_B)).unwrap();
        let sid = s
            .upsert_session("one", "h-a", None, None, 1, 1, "running", None)
            .unwrap();
        s.name_session_work(sid, Some("LOC-1"), "SECRET-A billing")
            .unwrap()
            .0
            .id
    };
    let tid = format!("item:{local}");
    while rx.try_recv().is_ok() {}

    st::place(
        &store,
        &crate::service::view_scope::ViewScope::internal(),
        &tid,
        Some("Payments"),
        Some("SECRET-A note"),
        Some(0),
        "master",
    )
    .unwrap();
    let rule = st::rule_save(
        &store,
        &OrgScope::All,
        &RuleInput {
            name: "Pay".into(),
            conditions: RuleConditions {
                key_prefix: Some("LOC".into()),
                ..Default::default()
            },
            group: "Payments".into(),
            ..Default::default()
        },
    )
    .unwrap();
    st::rule_delete(&store, &OrgScope::All, rule.id, Some(rule.version)).unwrap();
    let view = st::view_save(
        &store,
        &OrgScope::All,
        &ViewInput {
            name: "SECRET-A view".into(),
            ..Default::default()
        },
    )
    .unwrap();
    st::view_delete(&store, &OrgScope::All, view.id, Some(view.version)).unwrap();
    let imp = st::org_impact(&store, &iso_view(&OrgScope::All), &tid, Some(ORG_B)).unwrap();
    st::assign_org(
        &store,
        &iso_view(&OrgScope::All),
        &tid,
        Some(ORG_B),
        Some(&imp.impact_token),
    )
    .unwrap();

    let mut frames = Vec::new();
    while let Ok(m) = rx.try_recv() {
        if m.name == "work:changed" {
            frames.push(m);
        }
    }
    let whats: Vec<&str> = frames
        .iter()
        .map(|f| f.payload["what"].as_str().unwrap())
        .collect();
    assert_eq!(
        whats,
        ["placement", "rule", "rule", "view", "view", "org"],
        "one frame per structural write"
    );
    assert_eq!(
        frames[0].payload,
        json!({ "what": "placement", "task_id": tid })
    );
    assert_eq!(
        frames[1].payload,
        json!({ "what": "rule", "rule_id": rule.id })
    );
    assert_eq!(
        frames[3].payload,
        json!({ "what": "view", "view_id": view.id })
    );
    assert_eq!(frames[5].payload, json!({ "what": "org", "task_id": tid }));
    for f in &frames {
        let text = f.payload.to_string();
        assert!(!text.contains("SECRET"), "ids only: {text}");
    }

    let store = std::sync::Arc::new(store);
    for &who in EVERYONE {
        let c = who.caller();
        let kinds = fence_host_bound(&c, None);
        let asked = fence_host_bound(&c, Some(vec!["work".into()]));
        let view = {
            let s = store.lock().unwrap();
            c.view_scope(&s).unwrap()
        };
        for f in &frames {
            for k in [&kinds, &asked] {
                let delivered = matches(k.as_ref(), f) && fence_frame(&view, f, &store).is_some();
                // `!is_scoped()`, not `is_unbound()`, and the difference is
                // multi-user M1: a `work` frame names tracker tickets and
                // carries no session, so the ORG half is the whole fence —
                // `HOST_BOUND_HIDDEN_KINDS` keeps it off every host-bound
                // and org-bound stream and the person half has nothing to
                // say about a ticket. `is_unbound()` happens to answer the
                // same for this fixture's eight callers; saying what is
                // actually being asserted keeps it true when a ninth
                // carries a person and no org.
                assert_eq!(
                    delivered,
                    !c.is_scoped(),
                    "{who:?}: work:changed {} (asked {k:?})",
                    f.payload
                );
            }
        }
    }
}
