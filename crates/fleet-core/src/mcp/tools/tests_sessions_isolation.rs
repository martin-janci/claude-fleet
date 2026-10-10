//! The SESSION isolation matrix (multi-user M1, T14) — the acceptance gate
//! of session privacy, and the sibling of the ORG matrix next door.
//!
//! It is a **new file rather than an extension of `tests_isolation.rs`**
//! because that matrix covers a different boundary and cannot be widened to
//! this one: its coverage assertion builds `want` out of `WORK_ACTIONS` ∪
//! `WORK_LINK_ACTIONS` ∪ `AdminAction::NAMES` — the three work tools'
//! actions and nothing else — and its fixture gives every caller a visible
//! row on purpose, so that what refuses what stays legible. The two run
//! side by side and neither may be relaxed for the other.
//!
//! **What this pins.** One private session, owned by a person who is *not*
//! the hub's personal owner, against ten callers:
//!
//! | Who | What it is |
//! |---|---|
//! | `Owner` | the owning person's own paired device |
//! | `Watcher` | a second person's device, holding a `watch` grant |
//! | `Driver` | a third person's device, holding a `drive` grant |
//! | `Answerer` | a sixth person's device, holding an `answer` grant (Orbit Fleet 11.7) |
//! | `Stranger` | a fourth person's device, holding nothing |
//! | `HostPane` | the row's host's token, its request proving the row's pane |
//! | `HostNoPane` | the same token with no `X-Fleet-Pane` on the request |
//! | `HostElsewhere` | another host's token |
//! | `Master` | the master token — the hub's personal owner, i.e. the admin |
//! | `Legacy` | a paired device no pairing bound to a person |
//!
//! `Master` is the fixture's whole argument for rule 2 (privacy holds
//! against the org admin, with no override): the master token resolves to
//! the hub's personal owner (`Caller::view_scope`), and the row under test
//! is deliberately owned by somebody else, so every `Master` cell in the
//! matrix is a refusal. A fixture whose private row belonged to the
//! personal owner would have made the master its OWNER and pinned the
//! opposite of the rule.
//!
//! **A proven pane is a property of the caller's REQUEST, not of a call's
//! arguments** (R6-i). So `HostPane` and `HostNoPane` differ only in the
//! `pane` field of the caller fixture — one `X-Fleet-Pane` header — which
//! is what makes the "with a proven pane" column apply uniformly to every
//! tool in the derived set instead of to the three that once took a pane
//! argument.
//!
//! **Two checks run on every row, for every caller.**
//!
//! * **No leak.** The private row's `tmux_name`, `friendly_name`,
//!   `last_prompt`, `notes`, `tags`, `worktree_key` and `claude_session_id` are
//!   marker strings, and not one of them may appear in anything a caller
//!   that cannot see the row gets back — result or refusal sentence —
//!   after `call_tool`'s own result gate has run (T8). A marker the caller
//!   typed into its own arguments is exempt, exactly as in the org matrix.
//! * **The row's own expectation**, which is spec §4.3's tier table applied
//!   to the reach the tool threads, plus `same_as_unknown` wherever a
//!   refusal could otherwise be an existence oracle.
//!
//! **Where the covered set comes from.** `tests::session_addressed_tools()`
//! — the derivation in `tests.rs`, over
//! `FleetTools::tool_router_for_doc().list_all()` and the tools' input
//! SCHEMAS. **That function is the authority**, here and in
//! `tests::every_session_addressed_tool_declares_its_reach`; this file does
//! not derive a second set that could disagree with it. What it adds is
//! [`SESSION_REACHABLE_WITHOUT_A_KEY`], the handful of surfaces that reach
//! session rows while naming none in their schema — a schema derivation
//! cannot see those, and the plan's T14 names them one by one.
//!
//! A tool in either set with no `Matrix::row` call fails
//! `every_session_addressed_tool_has_a_matrix_row`; a tool with a row but no
//! arm in [`call`] panics with `no harness arm for …` rather than passing
//! quietly. Both are deliberate: the per-tool arm is what turns a forgotten
//! tool into a failure instead of a silent gap.

use super::*;
use crate::ipc_error::codes;
use crate::store::{GrantRecipient, GRANT_ANSWER, GRANT_DRIVE, GRANT_WATCH};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The private row's content, as marker strings (spec §4.3, *What counts as
/// content*). Every one of these is a field that table names.
const LEAK_TMUX: &str = "leaktmuxname";
const LEAK_FRIENDLY: &str = "LEAKFRIENDLY";
const LEAK_PROMPT: &str = "LEAKPROMPT";
const LEAK_NOTES: &str = "LEAKNOTES";
const LEAK_TAG: &str = "LEAKTAG";
/// Spec §4.3's content table names `cwd`, and `sessions` has no such
/// column: a session's working directory is its WORKTREE's path, which is a
/// property of a checkout rather than of a row, and `list_worktrees` serves
/// it to anybody. The row's own answer to "where does this work live" is
/// `worktree_key`, which the same table names in the same breath, so the
/// marker sits there.
const LEAK_WORKTREE_KEY: &str = "LEAKWORKTREEKEY";
const LEAK_CLAUDE: &str = "LEAKCLAUDEID";

const MARKERS: &[&str] = &[
    LEAK_TMUX,
    LEAK_FRIENDLY,
    LEAK_PROMPT,
    LEAK_NOTES,
    LEAK_TAG,
    LEAK_WORKTREE_KEY,
    LEAK_CLAUDE,
];

/// The four people's names. The hub's own personal owner is already called
/// `owner` (migration 100), so these are deliberately not role words.
const PERSON_OWNER: &str = "ada";
const PERSON_WATCHER: &str = "bob";
const PERSON_DRIVER: &str = "cho";
const PERSON_STRANGER: &str = "dee";
const PERSON_ANSWERER: &str = "fay";
/// The person the sharing rows grant to. It is **not** one of the nine
/// callers on purpose: the owner's own `session_share` row succeeds, so
/// granting to `dee` would turn the Stranger column into a watcher for every
/// row after it — which is exactly how a matrix comes to measure nothing.
const PERSON_SPARE: &str = "eve";

/// The row's host, and a second host so a per-host token elsewhere is a
/// caller rather than a hypothesis.
const HOST: &str = "h-row";
const FAR: &str = "h-far";

/// The work key the row is linked under.
const WORK_KEY: &str = "AA-1";

/// The pane the row's agent is standing in — written into
/// `sessions.tmux_pane_id`, which is what a provisioned host's
/// `X-Fleet-Pane` header resolves against (`Store::find_session_by_pane`).
const PANE: &str = "%41";

/// The pane of the UNCLAIMED row, so "claiming needs proof of THIS pane"
/// can be told apart from "a token on the right host" (rule 6).
const PANE_UNCLAIMED: &str = "%42";

/// The unclaimed row's tmux name. An `unclaimed` row leaks no metadata
/// either — rule 6 allows a per-host COUNT and nothing else — so it gets a
/// marker of its own rather than borrowing the private row's.
const UNCLAIMED_NAME: &str = "unclaimedmark";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    Owner,
    Watcher,
    Driver,
    Answerer,
    Stranger,
    HostPane,
    HostNoPane,
    HostElsewhere,
    Master,
    Legacy,
}

const EVERYONE: &[Who] = &[
    Who::Owner,
    Who::Watcher,
    Who::Driver,
    Who::Answerer,
    Who::Stranger,
    Who::HostPane,
    Who::HostNoPane,
    Who::HostElsewhere,
    Who::Master,
    Who::Legacy,
];

impl Who {
    fn caller(self, fx: &Fx) -> Caller {
        let device = |person: Option<i64>| Caller {
            api: None,
            host_alias: None,
            client: Some(crate::mcp::auth::ClientRef {
                id: 21,
                name: "phone".into(),
                trusted: true,
                org_id: None,
                person_id: person,
            }),
            mode: TokenMode::Full,
            pane: None,
            // As `auth::resolve_token` sets it: together with the person, so
            // a caller the resolver could not produce is not produced here
            // either.
            is_personal_owner: person == Some(fx.admin),
        };
        let host = |alias: &str, pane: Option<&str>| Caller {
            api: None,
            host_alias: Some(alias.into()),
            client: None,
            mode: TokenMode::Full,
            pane: pane.map(str::to_string),
            is_personal_owner: false,
        };
        match self {
            Who::Owner => device(Some(fx.owner)),
            Who::Watcher => device(Some(fx.watcher)),
            Who::Driver => device(Some(fx.driver)),
            Who::Answerer => device(Some(fx.answerer)),
            Who::Stranger => device(Some(fx.stranger)),
            Who::HostPane => host(HOST, Some(PANE)),
            Who::HostNoPane => host(HOST, None),
            Who::HostElsewhere => host(FAR, Some("%99")),
            Who::Master => Caller::master(),
            Who::Legacy => device(None),
        }
    }

    fn is_host(self) -> bool {
        matches!(self, Who::HostPane | Who::HostNoPane | Who::HostElsewhere)
    }

    /// May this caller read the private row's content at all? Everything
    /// else must never see a marker, in a result or in a refusal.
    ///
    /// `HostPane` is in the list because §4.4 clause 2 puts it there: the
    /// agent inside a fleet-started (therefore `private`) session reaches
    /// its own row, which is the whole reason the pane proof exists.
    fn may_read_the_row(self) -> bool {
        matches!(
            self,
            Who::Owner | Who::Watcher | Who::Driver | Who::Answerer | Who::HostPane
        )
    }
}

/// The outcome one cell expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Out {
    /// The session GATE did not refuse. The call may still fail for a reason
    /// that has nothing to do with access — no SSH to a fixture host, a
    /// `tool_use_id` the row never recorded, a conversation it never ran —
    /// and that is not this matrix's business.
    Pass,
    /// The session gate refused, with this code AND one of its own
    /// sentences.
    Gate(&'static str),
    /// The session gate refused with ONE OF these codes, and one of its own
    /// sentences. Used where two gate paths answer the same caller
    /// differently and both satisfy the spec — see [`tier`]'s
    /// `HostElsewhere` row.
    GateAny(&'static [&'static str]),
    /// Refused with exactly this code, by something that is not the session
    /// gate: a pre-gate policy fence, or a tool whose own refusal is
    /// deliberately shaped differently (`delete_worktree`'s
    /// `E_WORKTREE_BUSY`).
    Code(&'static str),
}

/// **The session gate's own refusal sentences** — the thing `Pass` and
/// `Gate` are really about.
///
/// Keying on CODES alone is not good enough in either direction, and both
/// failures are the kind this milestone keeps producing. Several tools
/// answer `E_NOTFOUND` legitimately for a sub-resource the row does not
/// have (`session_tool_detail`'s `tool_use_id`, `session_conversation`'s
/// conversation), so a `Pass` cell that only checked the code would have
/// had to be relaxed for a refusal the gate never issued. And an
/// `E_FORBIDDEN` from somewhere else entirely would have satisfied a
/// refusal cell while the gate did nothing — a gate returning the widest
/// answer having examined nothing is exactly what a matrix like this is
/// for.
///
/// Each fragment is one of the three refusals in
/// `support.rs::person_sees` plus `require_host`'s, quoted from it.
const GATE_SENTENCES: &[&str] = &[
    // `person_sees`' E_FORBIDDEN.
    "and this access does not carry it",
    // `person_sees`' E_PANE_UNPROVEN.
    "proves no pane of it",
    // `require_host`, the org/host half, which answers first.
    "this token is bound to",
];

/// The two things the gate refuses BY NAME with `{what} {id} not found`: a
/// session row (`person_sees`) and a task, whose own gate
/// (`support.rs::task_visible_at`) is the same rule read through the
/// `tasks` row that names a requester and a worker.
const GATE_NOT_FOUND_SUBJECTS: &[&str] = &["session", "task"];

/// Did the session gate refuse this answer?
fn gate_refused(a: &Answer) -> bool {
    let t = text(a);
    GATE_SENTENCES.iter().any(|f| t.contains(f))
        || GATE_NOT_FOUND_SUBJECTS
            .iter()
            .any(|what| says_not_found(t, what))
}

/// The gate's `E_NOTFOUND`, which is the bare sentence `{what} {id} not
/// found` — matched by shape rather than by the words "not found", which
/// half the API says for its own reasons.
fn says_not_found(t: &str, what: &str) -> bool {
    let head = format!("{what} ");
    t.split(&head).skip(1).any(|rest| {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        digits > 0 && rest[digits..].starts_with(" not found")
    })
}

/// Does this answer name session `id`? Matched with a digit boundary, so
/// row 1 is not found inside row 10 — the kind of substring accident that
/// makes an absence assertion pass for the wrong reason.
fn mentions_session(t: &str, id: i64) -> bool {
    let needle = format!("\"session_id\":{id}");
    t.split(&needle)
        .skip(1)
        .any(|rest| !rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// Spec §4.3's tier table, as the nine cells of a row-addressed tool.
///
/// This is the matrix's definition of right, and it is read off the spec,
/// not off the code:
///
/// * the owner reaches every tier;
/// * a `watch` grantee reads and is refused `drive` and `own`
///   (invariant 5: `own` is a tier no grant confers; and a watch grant that
///   reached a pane write would be a watch silently conferring drive);
/// * a `drive` grantee reads and drives and is refused `own`;
/// * a stranger is told nothing — `E_NOTFOUND`, exactly as an id that does
///   not exist, because a session's metadata IS its content;
/// * the row's host token with the pane proven reads and drives (§4.4
///   clause 2) and never owns — the proof says "I am standing in this
///   session", never "this session is mine";
/// * the same token with no pane proven gets `E_PANE_UNPROVEN`: it is on
///   the right host, so the refusal may say what is missing, and it says
///   nothing about whose the row is;
/// * a token bound to another host is refused — and the refusal has TWO
///   shapes, because the two gate wrappers differ and the matrix records it
///   rather than smoothing it over. `resolve_row_and_gate` applies
///   `require_host` first, so the refusal is `E_FORBIDDEN` naming the
///   session's host; `resolve_row_person_gated` (`session_history`,
///   `peer_status`, every `repo_*`) does NOT apply `require_host` at all, so
///   `person_sees`' own host clause answers `E_NOTFOUND` instead. Both
///   satisfy the spec — the second is strictly less informative, since it
///   names neither the host nor the row — so the cell accepts either and
///   insists the SESSION GATE is what refused;
/// * the MASTER is a refusal at every tier — rule 2, privacy holds against
///   the org admin with no override, audited or not;
/// * a device no pairing bound to a person is a refusing scope (§4.3
///   invariant 7), not a privileged one.
fn tier(reach: Reach, who: Who) -> Out {
    match who {
        Who::Owner => Out::Pass,
        Who::Watcher => match reach {
            Reach::Read => Out::Pass,
            _ => Out::Gate(codes::E_FORBIDDEN),
        },
        // Orbit Fleet 11.7: reads, answers a dialog, and nothing wider.
        Who::Answerer => match reach {
            Reach::Read | Reach::Answer => Out::Pass,
            _ => Out::Gate(codes::E_FORBIDDEN),
        },
        Who::Driver => match reach {
            Reach::Own => Out::Gate(codes::E_FORBIDDEN),
            _ => Out::Pass,
        },
        Who::Stranger | Who::Master | Who::Legacy => Out::Gate(codes::E_NOTFOUND),
        Who::HostPane => match reach {
            Reach::Own => Out::Gate(codes::E_FORBIDDEN),
            _ => Out::Pass,
        },
        Who::HostNoPane => Out::Gate(codes::E_PANE_UNPROVEN),
        Who::HostElsewhere => Out::GateAny(&[codes::E_FORBIDDEN, codes::E_NOTFOUND]),
    }
}

/// Tools a per-host token is never a caller of. They are refused BEFORE the
/// session gate, by `enforce_admin` reading `guard::NOT_FOR_HOST_TOKENS`, so
/// a host caller's cell is `E_FORBIDDEN` whatever the tier says — and
/// `the_pre_gate_policy_fences_are_what_the_cells_assume` holds every name
/// here to that table rather than to behaviour.
///
/// Two different reasons, both worth keeping legible:
///
/// * the five person-facing SHARING surfaces, because a per-host token
///   proves no person and so can be neither an owner nor a grantee
///   (`sharing.rs`'s header, spec §4.3), and presence (11.7b), which
///   reports a PERSON looking at a session;
/// * `list_downloads`, because listing and removing sent files is a
///   person's half of the downloads feature — a host's Claude only SENDS one
///   (`send_file`, which is deliberately NOT here: the session's own agent
///   is that tool's headline caller). `library` (Control's Library, 9.7) is
///   the index beside it, a person's for the same reason.
const NEVER_A_HOST_TOKENS: &[&str] = &[
    "session_share",
    "session_unshare",
    "session_narrow",
    "session_access",
    "my_grants",
    "session_presence",
    "session_ask_access",
    "access_requests",
    "list_downloads",
    "library",
    // The Automation screen's Runs list (Orbit Fleet 8.3): a person's.
    "runs",
];

/// The one tool a per-host token is the only caller of (`Access::HostToken`,
/// multi-user M1 T12): the operator's own claim is `fleet-hub session
/// claim`, on the hub machine.
const ONLY_A_HOST_TOKENS: &[&str] = &["session_claim"];

/// Served to a `TokenMode::Peer` token and to nothing else, so every caller
/// in this matrix is refused it by `enforce_mode`.
const ONLY_A_PEER: &[&str] = &["peer_exchange"];

/// **Session-REACHABLE tools that name no session in their schema.**
///
/// `tests::session_addressed_tools()` is derived from the input schemas, so
/// a tool that reaches session rows without naming one is invisible to it —
/// and `fleet_health` and `usage_report` were invisible to both of M1's
/// original lists for exactly that reason. A row here is a claim that the
/// tool's ANSWER is built out of session rows, so the matrix's leak check
/// is the thing that holds it; the plan's T14 names each of them.
const SESSION_REACHABLE_WITHOUT_A_KEY: &[(&str, &str)] = &[
    (
        "fleet_health",
        "a fleet-wide roll-up built over session rows: Attention items name \
         sessions, and the counts are counts of them",
    ),
    (
        "usage_report",
        "per-host and per-session token spend, keyed on rows this caller may \
         not see",
    ),
    (
        "broadcast_prompt",
        "no session argument at all: the fan-out is cut by \
         `BroadcastFilter::view`, which keeps only the rows this caller may \
         drive",
    ),
    (
        "list_worktrees",
        "a worktree page whose rows carry the sessions living in them",
    ),
    ("list_host_worktrees", "the same, read live off one host"),
    (
        "discover_lost_sessions",
        "it scans a host's Claude transcripts for conversations fleet has no \
         row for, which is past work of whoever ran them",
    ),
    (
        "my_grants",
        "the caller's own person and every live grant TO them: it takes no \
         parameters, so there is nothing to address a session with, and \
         `None` must answer an EMPTY list rather than every grant",
    ),
];

struct Fx {
    t: FleetTools,
    /// The hub's personal owner — the admin, and `Master`'s person. It owns
    /// nothing in this fixture on purpose.
    admin: i64,
    owner: i64,
    watcher: i64,
    driver: i64,
    answerer: i64,
    stranger: i64,
    /// The private row every tier row addresses.
    row: i64,
    /// Reconcile-discovered, owned by nobody: `session_claim`'s target and
    /// §4.4 clause 1's.
    unclaimed: i64,
    /// A second unclaimed row on the same host, whose pane nobody proves.
    unclaimed_other: i64,
    project: i64,
    worktree: i64,
    /// A task whose requester and worker are both `row`.
    task: i64,
    /// A work link on `row`, under [`WORK_KEY`].
    link: i64,
    /// A `downloads` row taken OUT of `row` — a file `send_file` copied off
    /// the owner's host, in state `ready`, so `list_downloads` has something
    /// to hide. Without it that row would assert over an empty page and
    /// measure nothing.
    download: i64,
    /// A `library_items` row placed beside `row`, so `library`'s list has
    /// something to hide.
    library_item: i64,
}

/// Set the process-global downloads directory, which
/// `service::downloads::send` demands (`dir()?`) **before** it resolves the
/// session.
///
/// Without it every caller gets `E_UNSUPPORTED` from a line above the gate
/// and the whole `send_file` row would pass while measuring nothing — and
/// which way it went would depend on whether `service::downloads`' own tests
/// happened to run first in this process, since the directory is a
/// `OnceLock`. The same shape as `downloads_tests::test_dir`: one temp
/// directory per process, kept.
fn downloads_dir() -> &'static std::path::Path {
    static D: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    D.get_or_init(|| {
        let tmp = tempfile::tempdir().unwrap().keep();
        let s = Store::open_in_memory().unwrap();
        crate::service::downloads::init(&tmp, &s).unwrap()
    })
}

fn fixture() -> Fx {
    downloads_dir();
    let s = Store::open_in_memory().unwrap();
    // No `local` host, for the reason the org matrix states: with one,
    // `list_sessions`' reconcile pass adopts whatever tmux sessions the
    // developer running the suite happens to have open.
    s.set_setting(crate::service::hub::SETTING_LOCAL_HOST, "false")
        .unwrap();
    for h in [HOST, FAR] {
        s.upsert_host(h).unwrap();
    }
    s.conn_for_test()
        .execute("UPDATE hosts SET reachable = 1", [])
        .unwrap();

    // Five people. The personal owner is the ADMIN and owns nothing: the
    // row under test belongs to `owner`, so `Master` is a stranger to it
    // and rule 2 is what the Master column measures.
    //
    // Five also means `Store::sole_enabled_person` answers `None`, so the
    // single-person carve-out (`ViewScope::sole_persons_unclaimed`) is off
    // for every caller here — this matrix is about a SHARED hub. The
    // carve-out has its own tests.
    let admin = s.personal_owner_id().unwrap().expect("096 mints one");
    // Migration 098 mints the personal owner under the name `owner`, so the
    // four people below are named for their letters rather than their roles:
    // `create_person("owner", …)` is `E_EXISTS` on any hub.
    let owner = s.create_person(PERSON_OWNER, None).unwrap().id;
    let watcher = s.create_person(PERSON_WATCHER, None).unwrap().id;
    let driver = s.create_person(PERSON_DRIVER, None).unwrap().id;
    let stranger = s.create_person(PERSON_STRANGER, None).unwrap().id;
    let answerer = s.create_person(PERSON_ANSWERER, None).unwrap().id;
    s.create_person(PERSON_SPARE, None).unwrap();
    assert_eq!(
        s.sole_enabled_person().unwrap(),
        None,
        "seven people: the single-person carve-out must be off for everyone"
    );

    let project = s.upsert_project("acme", "api", "/src/acme").unwrap();
    let worktree = s
        .upsert_worktree_on(HOST, project, "wt", "/src/acme/wt", Some("main"))
        .unwrap();

    let row = s
        .upsert_session(
            LEAK_TMUX,
            HOST,
            Some(project),
            Some(worktree),
            1,
            1,
            "running",
            None,
        )
        .unwrap();
    let unclaimed = s
        .upsert_session(UNCLAIMED_NAME, HOST, None, None, 1, 1, "running", None)
        .unwrap();
    // A second unclaimed row on the same host, so "the pane proves the row"
    // can be told from "the token is on the host".
    let unclaimed_other = s
        .upsert_session(
            "second-unclaimed-row",
            HOST,
            None,
            None,
            1,
            1,
            "running",
            None,
        )
        .unwrap();
    s.conn_for_test()
        .execute(
            "UPDATE sessions SET tmux_pane_id = ?2 WHERE id = ?1",
            rusqlite::params![unclaimed, PANE_UNCLAIMED],
        )
        .unwrap();
    s.claim_if_unclaimed(row, Some(owner)).unwrap();
    // Every field spec §4.3's content table names, as a marker. Written in
    // one statement rather than through seven setters: what matters is that
    // the row carries them, not which API put them there.
    s.conn_for_test()
        .execute(
            "UPDATE sessions SET tmux_pane_id = ?2, friendly_name = ?3, last_prompt = ?4, \
                    notes = ?5, tags = ?6, worktree_key = ?7, claude_session_id = ?8, \
                    started_at = 1, turn_seq = 3 \
              WHERE id = ?1",
            rusqlite::params![
                row,
                PANE,
                LEAK_FRIENDLY,
                LEAK_PROMPT,
                LEAK_NOTES,
                format!("[{:?}]", LEAK_TAG),
                LEAK_WORKTREE_KEY,
                LEAK_CLAUDE
            ],
        )
        .unwrap();
    {
        let got = s.get_session_by_id(row).unwrap().unwrap();
        assert_eq!(got.owner_person_id, Some(owner), "the row is the owner's");
        assert_eq!(got.visibility, crate::store::VISIBILITY_PRIVATE);
        assert_eq!(
            s.get_session_by_id(unclaimed).unwrap().unwrap().visibility,
            crate::store::VISIBILITY_UNCLAIMED
        );
    }
    // The two grants, made by the owner — the only caller who may make one.
    s.grant_session(row, GrantRecipient::Person(watcher), GRANT_WATCH, owner)
        .unwrap();
    s.grant_session(row, GrantRecipient::Person(answerer), GRANT_ANSWER, owner)
        .unwrap();
    s.grant_session(row, GrantRecipient::Person(driver), GRANT_DRIVE, owner)
        .unwrap();

    let task = s
        .insert_task(Some(row), Some(row), "work on it", "n1")
        .unwrap()
        .id;
    // A work link on the row, so the `own`-tier sweep can address
    // `work_link`'s conversation arms by `link_id` instead of skipping them.
    let link = s
        .link_session_work(row, crate::store::WorkTarget::Key(WORK_KEY), "manual")
        .unwrap()
        .id;

    // One file sent out of the private row. `session_name` is the row's own
    // `tmux_name` — what `downloads::send` records — so it is a MARKER, and
    // the leak check over `list_downloads` is a real one rather than a walk
    // over an empty page. `ready`, because a ready row is the dangerous one:
    // its bytes are what `GET /downloads/<id>` serves.
    let download = s
        .insert_download(&crate::store::NewDownload {
            host_alias: HOST,
            session_id: Some(row),
            session_name: Some(LEAK_TMUX),
            org_id: None,
            path: "/src/acme/wt/out/report.pdf",
            name: "report.pdf",
            size: 11,
            source: crate::service::downloads::SOURCE_AGENT,
            note: None,
        })
        .unwrap()
        .id;
    s.finish_download(download, "da39a3ee").unwrap();
    // One file a person put beside the private row (Control's Library):
    // its `session_name` is the same MARKER.
    let library_item = s
        .insert_library_item(&crate::store::NewLibraryItem {
            kind: crate::service::library::KIND_UPLOAD,
            host_alias: HOST,
            session_id: Some(row),
            session_name: Some(LEAK_TMUX),
            org_id: None,
            path: "/src/acme/wt/.claude-fleet-attachments/spec.pdf",
            name: "spec.pdf",
            size: Some(7),
        })
        .unwrap()
        .id;

    let t = FleetTools::new(
        Arc::new(Mutex::new(s)),
        Arc::new(SshClient::new()),
        CancellationRegistry::new(),
        Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
        McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {})),
    );
    Fx {
        t,
        admin,
        owner,
        watcher,
        driver,
        answerer,
        stranger,
        row,
        unclaimed,
        unclaimed_other,
        project,
        worktree,
        task,
        link,
        download,
        library_item,
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

/// One call the way `call_tool` makes it: `enforce_mode`, `enforce_admin`,
/// the tool, then the RESULT GATE over whatever came back (T8's
/// `fence_result_via`, which `call_tool` runs unconditionally). The leak
/// check therefore measures what a caller really receives, net included.
///
/// The hand-written dispatch is the point of this function: `other =>
/// panic!` is what turns a tool nobody wrote an arm for into a failure.
async fn call(fx: &Fx, who: Who, tool: &str, args: Value) -> Answer {
    let c = who.caller(fx);
    enforce_mode(&c, tool)
        .and_then(|()| enforce_admin(&c, tool))
        .map_err(|e| e.message.to_string())?;
    let ext = Extension(c.clone());
    // One arm per tool, each deserialising into that tool's OWN parameter
    // type: a helper closure cannot, since the type differs per arm.
    macro_rules! p {
        () => {
            Parameters(serde_json::from_value(args).expect("the row's args parse"))
        };
    }
    let r = match tool {
        // ---- lifecycle.rs -------------------------------------------------
        "kill_session" => fx.t.kill_session(ext, p!()).await,
        "shell_terminals" => fx.t.shell_terminals(ext, p!()).await,
        "safe_kill_session" => fx.t.safe_kill_session(ext, p!()).await,
        "rename_session" => fx.t.rename_session(ext, p!()).await,
        "set_friendly_name" => fx.t.set_friendly_name(ext, p!()).await,
        "touch_session_viewed" => fx.t.touch_session_viewed(ext, p!()).await,
        "restart_session" => fx.t.restart_session(ext, p!()).await,
        "rewind_conversation" => fx.t.rewind_conversation(ext, p!()).await,
        "spawn_review" => fx.t.spawn_review(ext, p!()).await,
        "repair_session" => fx.t.repair_session(ext, p!()).await,
        "move_session" => fx.t.move_session(ext, p!()).await,
        "resolve_move" => fx.t.resolve_move(ext, p!()).await,
        // ---- messaging.rs -------------------------------------------------
        "send_prompt" => fx.t.send_prompt(ext, p!()).await,
        "queue_prompt" => fx.t.queue_prompt(ext, p!()).await,
        "queued_prompts" => fx.t.queued_prompts(ext, p!()).await,
        "broadcast_prompt" => fx.t.broadcast_prompt(ext, p!()).await,
        "session_history" => fx.t.session_history(ext, p!()).await,
        "session_conversations" => fx.t.session_conversations(ext, p!()).await,
        "send_message" => fx.t.send_message(ext, p!()).await,
        "wait_for_reply" => fx.t.wait_for_reply(ext, p!()).await,
        // ---- forms.rs -----------------------------------------------------
        "ask" => fx.t.ask(ext, p!()).await,
        "inbox" => fx.t.inbox(ext, p!()).await,
        "peer_status" => fx.t.peer_status(ext, p!()).await,
        "peer_exchange" => fx.t.peer_exchange(ext, p!()).await,
        // ---- orchestration.rs ---------------------------------------------
        "wait_for_session" => fx.t.wait_for_session(ext, p!()).await,
        "session_transcript" => fx.t.session_transcript(ext, p!()).await,
        "session_conversation" => fx.t.session_conversation(ext, p!()).await,
        "session_tool_detail" => fx.t.session_tool_detail(ext, p!()).await,
        "session_summary_since" => fx.t.session_summary_since(ext, p!()).await,
        "run_prompt" => fx.t.run_prompt(ext, p!()).await,
        "dispatch_task" => fx.t.dispatch_task(ext, p!()).await,
        "wait_for_task" => fx.t.wait_for_task(ext, p!()).await,
        "list_tasks" => fx.t.list_tasks(ext, p!()).await,
        "cancel_task" => fx.t.cancel_task(ext, p!()).await,
        "set_session_tags" => fx.t.set_session_tags(ext, p!()).await,
        "decide_related_session" => fx.t.decide_related_session(ext, p!()).await,
        "work" => fx.t.work(ext, p!()).await,
        "work_link" => fx.t.work_link(ext, p!()).await,
        // ---- repo.rs ------------------------------------------------------
        "list_worktrees" => fx.t.list_worktrees(ext, p!()).await,
        "list_host_worktrees" => fx.t.list_host_worktrees(ext, p!()).await,
        "delete_worktree" => fx.t.delete_worktree(ext, p!()).await,
        "repo_changes" => fx.t.repo_changes(ext, p!()).await,
        "repo_tree" => fx.t.repo_tree(ext, p!()).await,
        "repo_file" => fx.t.repo_file(ext, p!()).await,
        "repo_diff" => fx.t.repo_diff(ext, p!()).await,
        "repo_blame" => fx.t.repo_blame(ext, p!()).await,
        "repo_log" => fx.t.repo_log(ext, p!()).await,
        "repo_branches" => fx.t.repo_branches(ext, p!()).await,
        "repo_commit" => fx.t.repo_commit(ext, p!()).await,
        "repo_commit_diff" => fx.t.repo_commit_diff(ext, p!()).await,
        "repo_branch_diff" => fx.t.repo_branch_diff(ext, p!()).await,
        "repo_range_diff" => fx.t.repo_range_diff(ext, p!()).await,
        // ---- session_ops.rs -----------------------------------------------
        "list_sessions" => fx.t.list_sessions(ext, p!()).await,
        "related_sessions" => fx.t.related_sessions(ext, p!()).await,
        "register_self" => fx.t.register_self(ext, p!()).await,
        "whoami" => fx.t.whoami(ext, p!()).await,
        "new_session" => fx.t.new_session(ext, p!()).await,
        "new_shell_session" => fx.t.new_shell_session(ext, p!()).await,
        "capture_session" => fx.t.capture_session(ext, p!()).await,
        "session_activity" => fx.t.session_activity(ext, p!()).await,
        "recreate_session" => fx.t.recreate_session(ext, p!()).await,
        "restore_host_sessions" => fx.t.restore_host_sessions(ext, p!()).await,
        "discover_lost_sessions" => fx.t.discover_lost_sessions(ext, p!()).await,
        "dismiss_ghost_session" => fx.t.dismiss_ghost_session(ext, p!()).await,
        "adopt_session" => fx.t.adopt_session(ext, p!()).await,
        "lost_target" => fx.t.lost_target(ext, p!()).await,
        "place_transcript" => fx.t.place_transcript(ext, p!()).await,
        "new_bg_session" => fx.t.new_bg_session(ext, p!()).await,
        // ---- sharing.rs ---------------------------------------------------
        "session_share" => fx.t.session_share(ext, p!()).await,
        "session_unshare" => fx.t.session_unshare(ext, p!()).await,
        "session_narrow" => fx.t.session_narrow(ext, p!()).await,
        "session_access" => fx.t.session_access(ext, p!()).await,
        "session_claim" => fx.t.session_claim(ext, p!()).await,
        "my_grants" => fx.t.my_grants(ext).await,
        "session_presence" => fx.t.session_presence(ext, p!()).await,
        "session_ask_access" => fx.t.session_ask_access(ext, p!()).await,
        "access_requests" => fx.t.access_requests(ext, p!()).await,
        // ---- downloads.rs -------------------------------------------------
        "send_file" => fx.t.send_file(ext, p!()).await,
        "list_downloads" => fx.t.list_downloads(ext, p!()).await,
        // ---- library.rs ---------------------------------------------------
        "library" => fx.t.library(ext, p!()).await,
        // ---- runs.rs ------------------------------------------------------
        "runs" => fx.t.runs(ext, p!()).await,
        // ---- fleet.rs -----------------------------------------------------
        "fleet_health" => fx.t.fleet_health(ext).await,
        "usage_report" => fx.t.usage_report(ext, p!()).await,
        other => panic!(
            "no harness arm for {other}: a session-addressed tool with no arm \
             is a tool this matrix says nothing about"
        ),
    };
    match r {
        Ok(mut res) => {
            // T8's gate, exactly as `call_tool` runs it: unconditional, for
            // every caller, over the result AND over an error's details.
            fx.t.fence_result_for(&c, &mut res);
            let body: String = res
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect();
            if res.is_error == Some(true) {
                return Err(body);
            }
            Ok(body)
        }
        Err(e) => Err(e.message.to_string()),
    }
}

/// Same code and same sentence with the id swapped: no existence oracle.
fn same_as_unknown(a: &Answer, unknown: &Answer, id: &str, unknown_id: &str) {
    assert_eq!(code(a), code(unknown), "{a:?} vs {unknown:?}");
    let swap_last = |t: &str, x: &str| match t.rfind(x) {
        Some(i) => format!("{}<X>{}", &t[..i], &t[i + x.len()..]),
        None => t.to_string(),
    };
    assert_eq!(
        swap_last(text(a), id),
        swap_last(text(unknown), unknown_id),
        "a row this caller may not see must read exactly as one that does not exist"
    );
}

/// Run one matrix row for every caller: the leak check, then the cell.
struct Matrix<'a> {
    fx: &'a Fx,
    covered: BTreeSet<String>,
}

impl Matrix<'_> {
    /// A row whose cells are given explicitly.
    async fn row(
        &mut self,
        tool: &str,
        args: impl Fn(&Fx, Who) -> Value,
        expect: impl Fn(Who) -> Out,
    ) {
        self.covered.insert(tool.to_string());
        for &who in EVERYONE {
            let asked = args(self.fx, who);
            let asked_text = asked.to_string();
            let a = call(self.fx, who, tool, asked).await;
            if !who.may_read_the_row() {
                for m in MARKERS.iter().filter(|m| !asked_text.contains(**m)) {
                    assert!(
                        !text(&a).contains(m),
                        "LEAK: {who:?} read {m:?} through {tool}: {}",
                        text(&a)
                    );
                }
            }
            match expect(who) {
                Out::Pass => assert!(
                    !gate_refused(&a),
                    "{who:?} must be past {tool}'s gate and the gate refused: {a:?}"
                ),
                Out::Gate(want) => {
                    assert_eq!(code(&a), want, "{who:?} on {tool}: {a:?}");
                    assert!(
                        gate_refused(&a),
                        "{who:?} on {tool} answered {want} but not from the session \
                         gate, so this cell proves nothing: {a:?}"
                    );
                }
                Out::GateAny(want) => {
                    assert!(
                        want.contains(&code(&a)),
                        "{who:?} on {tool} must be refused with one of {want:?}: {a:?}"
                    );
                    assert!(
                        gate_refused(&a),
                        "{who:?} on {tool} was refused but not by the session gate, \
                         so this cell proves nothing: {a:?}"
                    );
                }
                Out::Code(want) => assert_eq!(code(&a), want, "{who:?} on {tool}: {a:?}"),
            }
        }
    }

    /// A row addressed at the private row, at the reach its tool threads:
    /// the cells are spec §4.3's tier table ([`tier`]), with the two
    /// pre-gate policy fences applied.
    ///
    /// `reach` is cross-checked against the tool's own `SESSION_REACH` row
    /// in `tests.rs` — the table `every_session_addressed_tool_declares_its_reach`
    /// holds to the handlers' source — so a tool whose tier moves there and
    /// not here fails rather than being measured at the old level.
    async fn gated(&mut self, tool: &str, reach: Reach, args: impl Fn(&Fx, Who) -> Value) {
        assert_declared(tool, reach);
        self.row(tool, args, |who| cell(tool, reach, who)).await;
    }

    /// A row with its own cells, still cross-checked against
    /// `SESSION_REACH`: the tools whose refusal is deliberately NOT the
    /// gate's own (`new_session`'s worktree landing, a task's own not-found,
    /// a batch that reports a skip instead of refusing) still have to be
    /// measured at the tier their handler threads.
    async fn at(
        &mut self,
        tool: &str,
        reach: Reach,
        args: impl Fn(&Fx, Who) -> Value,
        expect: impl Fn(Who) -> Out,
    ) {
        assert_declared(tool, reach);
        self.row(tool, args, expect).await;
    }
}

/// The reach this matrix measures a tool at must be one its own
/// `SESSION_REACH` row declares — so a tier that moves there and not here
/// fails instead of being measured at the old level.
fn assert_declared(tool: &str, reach: Reach) {
    let declared: Vec<&str> = super::tests::SESSION_REACH
        .iter()
        .find(|(n, _)| *n == tool)
        .map(|(_, r)| r.to_vec())
        .unwrap_or_else(|| panic!("{tool} has no SESSION_REACH row"));
    let want = match reach {
        Reach::Read => "Read",
        Reach::Answer => "Answer",
        Reach::Drive => "Drive",
        Reach::Own => "Own",
    };
    assert!(
        declared.contains(&want),
        "this matrix measures {tool} at {want} and SESSION_REACH declares {declared:?}"
    );
}

/// [`tier`] with the two pre-gate policy fences: a per-host token never
/// calls a sharing surface, and nobody but a per-host token calls
/// `session_claim`. Both refuse in `enforce_admin`, before any row is
/// resolved.
fn cell(tool: &str, reach: Reach, who: Who) -> Out {
    if who.is_host() && NEVER_A_HOST_TOKENS.contains(&tool) {
        return Out::Code(codes::E_FORBIDDEN);
    }
    if !who.is_host() && ONLY_A_HOST_TOKENS.contains(&tool) {
        return Out::Code(codes::E_FORBIDDEN);
    }
    tier(reach, who)
}

// ─────────────────────────────────────────────────────────────────────────
// The matrix
// ─────────────────────────────────────────────────────────────────────────

async fn run_matrix() {
    let fx = fixture();
    let mut m = Matrix {
        fx: &fx,
        covered: BTreeSet::new(),
    };
    let row = |fx: &Fx, _: Who| json!({ "session_id": fx.row });

    // ---- the `own` tier: spec §4.3 invariant 5's list ---------------------
    m.gated("kill_session", Reach::Own, row).await;
    m.gated("safe_kill_session", Reach::Own, row).await;
    m.gated("restart_session", Reach::Own, row).await;
    m.gated("recreate_session", Reach::Own, row).await;
    m.gated("shell_terminals", Reach::Own, row).await;
    m.gated(
        "move_session",
        Reach::Own,
        |fx, _| json!({ "session_id": fx.row, "target_host_alias": FAR, "dry_run": true }),
    )
    .await;
    m.gated(
        "resolve_move",
        Reach::Own,
        |fx, _| json!({ "session_id": fx.row, "action": "finish" }),
    )
    .await;
    m.gated(
        "spawn_review",
        Reach::Own,
        |fx, _| json!({ "source_session_id": fx.row, "prompt": "review it" }),
    )
    .await;
    m.gated(
        "rewind_conversation",
        Reach::Own,
        |fx, _| json!({ "session_id": fx.row, "mode": "fork" }),
    )
    .await;
    // `restore_host_sessions` is `recreate_session` in bulk and it never
    // REFUSES: `gate_restore_plan` puts every planned session through the
    // same `Reach::Own`, and one the caller may not restore becomes a `skip`
    // entry reading `not found on this host` — the no-oracle shape, since
    // the batch is addressed by HOST. So the cells say "past the gate", and
    // the sweep below is what holds the refusal: nothing but the owner may
    // actually restore the row.
    m.at(
        "restore_host_sessions",
        Reach::Own,
        |fx, _| json!({ "host_alias": HOST, "session_ids": [fx.row], "dry_run": true }),
        |who| match who {
            // `require_host` on the NAMED host answers before the plan.
            Who::HostElsewhere => Out::Gate(codes::E_FORBIDDEN),
            _ => Out::Pass,
        },
    )
    .await;
    for who in [
        Who::Watcher,
        Who::Driver,
        Who::Stranger,
        Who::HostPane,
        Who::HostNoPane,
        Who::Master,
        Who::Legacy,
    ] {
        let a = call(
            &fx,
            who,
            "restore_host_sessions",
            json!({ "host_alias": HOST, "session_ids": [fx.row], "dry_run": true }),
        )
        .await;
        assert!(
            text(&a).contains(crate::service::sessions::NOT_ON_THIS_HOST),
            "{who:?} is not the owner, so the batch must SKIP the row rather \
             than plan a restart of it: {a:?}"
        );
    }
    m.gated(
        "rename_session",
        Reach::Own,
        |fx, _| json!({ "session_id": fx.row, "new_name": "renamed" }),
    )
    .await;
    m.gated(
        "set_session_tags",
        Reach::Own,
        |fx, _| json!({ "session_id": fx.row, "tags": ["t"] }),
    )
    .await;
    m.gated(
        "decide_related_session",
        Reach::Own,
        |fx, _| json!({ "session_id": fx.row, "run_id": 1, "linked": true }),
    )
    .await;
    // `delete_worktree` is `Own` on every OCCUPANT of the tree, and it is
    // addressed by `worktree_id` — so its refusal is the WORKTREE's, never
    // the occupant's. A caller that cannot see the occupant gets
    // `E_WORKTREE_BUSY`, which is the shape a merely-busy tree answers with,
    // rather than the gate's own `session {id} not found` — walking worktree
    // ids would otherwise tell anybody which trees hold a private session
    // and what its id is. A caller that CAN see it (a grantee) keeps
    // `E_FORBIDDEN` and is told the tier.
    m.row(
        "delete_worktree",
        |fx, _| json!({ "worktree_id": fx.worktree, "force": true }),
        |who| match who {
            Who::Owner => Out::Pass,
            Who::Watcher | Who::Driver | Who::Answerer | Who::HostPane => {
                Out::Code(codes::E_FORBIDDEN)
            }
            _ => Out::Code(codes::E_WORKTREE_BUSY),
        },
    )
    .await;
    // The four sharing writes and the grant list: `Own`, because this is
    // where "a grantee cannot grant on" is enforced (invariant 2).
    for tool in ["session_share", "session_unshare", "session_narrow"] {
        m.gated(
            tool,
            Reach::Own,
            |fx, _| json!({ "session_id": fx.row, "person": PERSON_SPARE, "level": "watch" }),
        )
        .await;
    }
    m.gated("session_access", Reach::Own, row).await;
    // Presence (11.7b): a watch grantee may say it is looking; a stranger
    // gets the not-found every session tool answers, and learns nobody's
    // there.
    m.gated("session_presence", Reach::Read, row).await;
    // Gap plan G4.2: asking the owner for more is for someone the session is
    // shared with below drive. The gate is `Read` (a stranger learns
    // nothing); the store then refuses the owner (nobody to ask) and a
    // driver (already at the top).
    m.at(
        "session_ask_access",
        Reach::Read,
        |fx, _| json!({ "session_id": fx.row, "level": "drive" }),
        |who| match who {
            w if w.is_host() => Out::Code(codes::E_FORBIDDEN),
            Who::Owner | Who::Driver => Out::Code(codes::E_VALIDATE),
            w => tier(Reach::Read, w),
        },
    )
    .await;
    // The owner's list of asks names other people, so it is `Own`, like
    // `session_access`.
    m.gated(
        "access_requests",
        Reach::Own,
        |fx, _| json!({ "action": "list", "session_id": fx.row }),
    )
    .await;
    // `send_file` is the `own` tier, and the reason is the FILE's path
    // rather than anything about the session: `send_file { session_id, path }`
    // copies a file off the session's host at an UNCONSTRAINED absolute path
    // — `downloads::parse_stat`'s whole success condition is
    // `path.starts_with('/')`, with no canonicalisation against a root and no
    // `starts_with(worktree)` — and `GET /downloads/<id>` then hands the
    // BYTES to whoever `downloads::visible` admits. That is a subset of what
    // a terminal gives, which §4.3 invariant 5 says no grant ever confers, so
    // the confined `repo_file`'s `Reach::Read` is not the precedent it looks
    // like. (If somebody later confines the path to the session's tree,
    // `Read` becomes defensible and this row moves with it.)
    //
    // Its cells depart from [`tier`] three times, and each departure is
    // recorded here rather than smoothed into the tier it nearly matches:
    //
    // * **every refusal is `E_NOTFOUND`, never `E_FORBIDDEN`.**
    //   `downloads::send` has ONE `_ =>` arm answering
    //   `session {id} not found`, so a WATCHER and a DRIVER are told the row
    //   does not exist rather than told the level they lack. Stricter than
    //   §4.3, which would merely refuse them, and the no-oracle sweep below
    //   is what keeps that strictness honest instead of accidental;
    // * **`HostNoPane` is that same `E_NOTFOUND`**, not the
    //   `E_PANE_UNPROVEN` every `person_sees` path answers: the per-host arm
    //   is `sees_session_row(..).is_visible()`, a boolean with no third "on
    //   the right host, proving no pane" answer to return. Stricter again,
    //   and strictly less informative;
    // * **`HostPane` PASSES**, which `tier(Own, ..)` refuses. That is
    //   deliberate and is `send`'s second arm: the session's own Claude is
    //   this tool's headline caller (`whoami` hands it the `session_id`), and
    //   the pane proof never reaches `may_own`, so its gate is §4.4 clauses 1
    //   and 2 through `sees_session_row` — its own host, and the one pane
    //   this request proves. `SESSION_REACH`'s row and `reaches_in`'s
    //   `downloads::send(` clause in `tests.rs` both say so.
    m.at(
        "send_file",
        Reach::Own,
        |fx, _| json!({ "session_id": fx.row, "path": "out/report.pdf" }),
        |who| match who {
            Who::Owner | Who::HostPane => Out::Pass,
            _ => Out::Gate(codes::E_NOTFOUND),
        },
    )
    .await;
    // No oracle on any of those seven refusals. The sentence is the gate's
    // one `session {id} not found`, which is byte-identical to the one an id
    // nobody ever used produces — so walking session ids with `send_file`
    // tells a watcher, a driver, a stranger, the master and a host token
    // elsewhere exactly nothing about which ids exist.
    for who in [
        Who::Watcher,
        Who::Driver,
        Who::Stranger,
        Who::HostNoPane,
        Who::HostElsewhere,
        Who::Master,
        Who::Legacy,
    ] {
        let hidden = call(
            &fx,
            who,
            "send_file",
            json!({ "session_id": fx.row, "path": "out/report.pdf" }),
        )
        .await;
        let unknown = call(
            &fx,
            who,
            "send_file",
            json!({ "session_id": 999999, "path": "out/report.pdf" }),
        )
        .await;
        same_as_unknown(&hidden, &unknown, &fx.row.to_string(), "999999");
    }
    // The umbrella's `own` arms (`resume`, `summarize`) are addressed by a
    // work key and a `link_id`, not by a session, and they answer
    // `summary::no_such_link` rather than naming a tier — a caller not
    // entitled to learn the conversation exists is not told that it does.
    // They are swept by `a_driver_is_refused_every_owner_only_tool` below,
    // and the per-ACTION tiers of both umbrellas are
    // `tests::WORK_ACTION_REACH` and `tests_isolation`'s own matrix. This
    // file measures `work_link` at its session-addressed `Drive` arm.

    // ---- the `drive` tier -------------------------------------------------
    // A task's cells, shared by `cancel_task` and `wait_for_task`: see the
    // comment above `cancel_task`.
    let task_cells = |reach: Reach| {
        move |who: Who| match who {
            Who::Owner | Who::HostPane | Who::Driver => Out::Pass,
            Who::Watcher | Who::Answerer => match reach {
                Reach::Read => Out::Pass,
                _ => Out::Gate(codes::E_FORBIDDEN),
            },
            _ => Out::Gate(codes::E_NOTFOUND),
        }
    };
    m.gated(
        "send_prompt",
        Reach::Drive,
        |fx, _| json!({ "session_id": fx.row, "prompt": "go" }),
    )
    .await;
    m.gated(
        "queue_prompt",
        Reach::Drive,
        |fx, _| json!({ "session_id": fx.row, "prompt": "go" }),
    )
    .await;
    // The list is the session's pending input, and `cancel` takes it back:
    // `drive` for both, so a watcher reads none of it.
    m.gated(
        "queued_prompts",
        Reach::Drive,
        |fx, _| json!({ "session_id": fx.row }),
    )
    .await;
    m.gated(
        "run_prompt",
        Reach::Drive,
        |fx, _| json!({ "session_id": fx.row, "prompt": "go", "timeout_s": 1 }),
    )
    .await;
    // Its recipient goes through `require_message_recipient`, which IS the
    // gate at `Reach::Drive`. `deliver` + `submit` is a PANE write by
    // another route and sits at exactly `send_prompt`'s level — refusing a
    // driver one while allowing the other would be theatre, and a watcher
    // must reach neither.
    m.gated("send_message", Reach::Drive, |fx, _| {
        json!({
            "from_session_id": fx.row,
            "to_session_id": fx.row,
            "body": "hello",
            "deliver": true,
            "submit": true
        })
    })
    .await;
    m.gated("dispatch_task", Reach::Drive, |fx, _| {
        json!({
            "worker_session_id": fx.row,
            "requester_session_id": fx.row,
            "prompt": "do it"
        })
    })
    .await;
    // A task is addressed by `task_id` and gated through the sessions it
    // names (`support.rs::task_visible_at`), so the not-found it answers is
    // the TASK's: a caller that cannot see the task's sessions cannot see
    // the task. There is no `require_host` in front of that check and no
    // pane clause behind it, so the two host callers that cannot see the row
    // read as an unknown task rather than as `E_PANE_UNPROVEN` — strictly
    // less than the gate says elsewhere, and recorded here rather than
    // smoothed over.
    m.at(
        "cancel_task",
        Reach::Drive,
        |fx, _| json!({ "task_id": fx.task }),
        task_cells(Reach::Drive),
    )
    .await;
    m.gated(
        "set_friendly_name",
        Reach::Drive,
        |fx, _| json!({ "session_id": fx.row, "friendly_name": "caption" }),
    )
    .await;
    m.gated("repair_session", Reach::Drive, row).await;
    m.gated("touch_session_viewed", Reach::Drive, row).await;
    m.gated("dismiss_ghost_session", Reach::Drive, row).await;
    m.gated("adopt_session", Reach::Own, row).await;
    m.gated("lost_target", Reach::Own, row).await;
    m.gated("register_self", Reach::Drive, row).await;
    m.gated("new_bg_session", Reach::Drive, |fx, _| {
        json!({
            "host_alias": HOST,
            "name": "bg",
            "prompt": "go",
            "requester_session_id": fx.row
        })
    })
    .await;
    // Landing a NEW pane in the worktree a private session is working in is
    // a write in that tree, which is why both are `Drive` on the occupant.
    // Both are addressed by `worktree_id`, so — exactly like
    // `delete_worktree` — their refusal is the TREE's and names no session:
    // `E_FORBIDDEN` with `LANDING_NOT_YOURS`, identical for a caller that
    // cannot see the occupant and one that can see it but may not drive it.
    // That is why these cells are `Code` and not `Gate`.
    let cannot_land = |who: Who| match who {
        Who::Owner | Who::Driver | Who::HostPane => Out::Pass,
        _ => Out::Code(codes::E_FORBIDDEN),
    };
    m.at(
        "new_session",
        Reach::Drive,
        |fx, _| {
            json!({
                "host_alias": HOST,
                "project_id": fx.project,
                "worktree_id": fx.worktree,
                "name": "fresh"
            })
        },
        cannot_land,
    )
    .await;
    m.at(
        "new_shell_session",
        Reach::Drive,
        |fx, _| {
            json!({
                "host_alias": HOST,
                "project_id": fx.project,
                "worktree_id": fx.worktree,
                "name": "shell"
            })
        },
        cannot_land,
    )
    .await;
    // `inbox` is the one surface that DEGRADES rather than refusing, and the
    // cells say so. `mark_read` defaults to TRUE, so `inbox { session_id }` —
    // the documented shape every pre-M1 client sends — asks for the write
    // without naming it; refusing it would leave a `watch` grant unable to
    // read an inbox at all. So the read is served at the WATCH tier and the
    // owner's unread cursor is left alone (`reaches_row`, and the cursor
    // itself is pinned by
    // `tests::the_inbox_gate_binds_the_master_and_mark_read_needs_drive`).
    // Its `SESSION_REACH` row is `["Drive", "Read"]` for the same reason,
    // which is why this row cannot go through `gated`.
    m.row(
        "inbox",
        |fx, _| json!({ "session_id": fx.row, "mark_read": true }),
        |who| tier(Reach::Read, who),
    )
    .await;
    // The tail of the umbrella: a per-session work-graph write.
    m.gated(
        "work_link",
        Reach::Drive,
        |fx, _| json!({ "action": "link", "session_id": fx.row, "key": WORK_KEY }),
    )
    .await;

    // ---- the `watch` tier -------------------------------------------------
    m.gated("capture_session", Reach::Read, row).await;
    m.gated("session_activity", Reach::Read, row).await;
    m.gated("session_history", Reach::Read, row).await;
    m.gated("session_conversations", Reach::Read, row).await;
    m.gated("session_conversation", Reach::Read, row).await;
    m.gated(
        "session_tool_detail",
        Reach::Read,
        |fx, _| json!({ "session_id": fx.row, "tool_use_id": "tu-1" }),
    )
    .await;
    m.gated("session_transcript", Reach::Read, row).await;
    m.gated(
        "session_summary_since",
        Reach::Read,
        |fx, _| json!({ "session_id": fx.row, "since": 0 }),
    )
    .await;
    m.gated(
        "wait_for_reply",
        Reach::Read,
        |fx, _| json!({ "session_id": fx.row, "timeout_s": 1 }),
    )
    .await;
    m.gated(
        "wait_for_session",
        Reach::Read,
        |fx, _| json!({ "session_id": fx.row, "until": "idle", "timeout_s": 1 }),
    )
    .await;
    m.at(
        "wait_for_task",
        Reach::Read,
        |fx, _| json!({ "task_id": fx.task, "timeout_s": 1 }),
        task_cells(Reach::Read),
    )
    .await;
    m.gated("peer_status", Reach::Read, row).await;
    m.gated(
        "work",
        Reach::Read,
        |fx, _| json!({ "action": "links", "session_id": fx.row }),
    )
    .await;
    // Every `repo_*` read is `watch` on the session whose worktree it opens.
    m.gated("repo_changes", Reach::Read, row).await;
    m.gated("repo_tree", Reach::Read, row).await;
    m.gated("repo_branches", Reach::Read, row).await;
    m.gated(
        "repo_file",
        Reach::Read,
        |fx, _| json!({ "session_id": fx.row, "path": "README.md" }),
    )
    .await;
    m.gated(
        "repo_diff",
        Reach::Read,
        |fx, _| json!({ "session_id": fx.row, "path": "README.md" }),
    )
    .await;
    m.gated(
        "repo_blame",
        Reach::Read,
        |fx, _| json!({ "session_id": fx.row, "path": "README.md" }),
    )
    .await;
    m.gated("repo_log", Reach::Read, row).await;
    m.gated(
        "repo_commit",
        Reach::Read,
        |fx, _| json!({ "session_id": fx.row, "hash": "HEAD" }),
    )
    .await;
    m.gated(
        "repo_commit_diff",
        Reach::Read,
        |fx, _| json!({ "session_id": fx.row, "hash": "HEAD", "path": "README.md" }),
    )
    .await;
    m.gated("repo_branch_diff", Reach::Read, row).await;
    m.gated(
        "repo_range_diff",
        Reach::Read,
        |fx, _| json!({ "session_id": fx.row, "path": "README.md", "range": "base" }),
    )
    .await;

    // ---- the claim path: rule 6 ------------------------------------------
    // `session_claim` is `Read` on an UNCLAIMED row, and only a per-host
    // token calls it at all. Claiming needs proof of host access, so the
    // pane column is what tells the two host callers apart — and the token
    // on another host is refused by the host fence.
    m.row(
        "session_claim",
        |fx, _| json!({ "session_id": fx.unclaimed, "person": PERSON_OWNER }),
        |who| match who {
            // §4.4 clause 1 makes an `unclaimed` row on the token's own host
            // VISIBLE whatever pane the request proves — that is what makes
            // the claim path reachable at all — and the claim itself then
            // needs the proof of THAT row's pane. `HostPane` stands in the
            // private row's pane, so it is refused this one; `HostNoPane`
            // proves no pane at all. Neither refusal is the gate's.
            Who::HostPane => Out::Code(codes::E_FORBIDDEN),
            Who::HostNoPane => Out::Code(codes::E_INVALID_STATE),
            Who::HostElsewhere => Out::Code(codes::E_FORBIDDEN),
            // Not a per-host token: `Access::HostToken` refuses it before
            // anything is resolved.
            _ => Out::Code(codes::E_FORBIDDEN),
        },
    )
    .await;

    // ---- the pages and aggregates: no row to gate ------------------------
    // Each of these answers a PAGE or a roll-up rather than gating one named
    // row, so the matrix's assertion on them IS the leak check: nobody who
    // cannot see the private row may find its content in the answer. None of
    // them refuses anybody for naming it.
    let open = |_: Who| Out::Pass;
    m.row("list_sessions", |_, _| json!({ "summary": false }), open)
        .await;
    // `fresh_for` names a READER session, and `resolve_reader` must answer
    // identically for an id that does not exist and one this caller may not
    // see — pinned below as well.
    m.row(
        "list_sessions",
        |fx, _| json!({ "summary": false, "fresh_for": fx.row }),
        open,
    )
    .await;
    m.row(
        "list_tasks",
        |fx, _| json!({ "requester_session_id": fx.row }),
        // A page cut by the caller's view scope: naming a row it cannot see
        // yields an empty page, never a refusal.
        open,
    )
    .await;
    // `related_sessions` is a FILTER rather than a gate, and its anchor
    // check answers with `orgs::not_found` — so a caller that cannot see the
    // anchor reads exactly as a missing anchor does, the host token on the
    // right host included (no `E_PANE_UNPROVEN`: this path never reaches
    // `person_sees`).
    m.row(
        "related_sessions",
        |fx, _| json!({ "session_id": fx.row }),
        |who| match who {
            Who::Owner | Who::Watcher | Who::Driver | Who::Answerer | Who::HostPane => Out::Pass,
            _ => Out::Gate(codes::E_NOTFOUND),
        },
    )
    .await;
    m.row(
        "whoami",
        |_, _| json!({ "tmux_name": LEAK_TMUX }),
        // A name this caller may not see matches nothing:
        // `find_session_by_tmux_name_scoped` resolves through the view
        // scope, so the answer is a miss and not a refusal of a row.
        |who| match who {
            Who::Owner | Who::Watcher | Who::Driver | Who::Answerer | Who::HostPane => Out::Pass,
            _ => Out::Code(codes::E_NOTFOUND),
        },
    )
    .await;
    // No oracle on a NAME either: a tmux name this caller may not see reads
    // exactly as one nobody ever used. `find_session_by_tmux_name_scoped`
    // resolves through the view scope, so the private row is not even among
    // the `E_AMBIGUOUS` candidates.
    for who in [Who::Stranger, Who::Master, Who::Legacy] {
        let hidden = call(&fx, who, "whoami", json!({ "tmux_name": LEAK_TMUX })).await;
        let unknown = call(&fx, who, "whoami", json!({ "tmux_name": "no-such-name" })).await;
        same_as_unknown(&hidden, &unknown, LEAK_TMUX, "no-such-name");
    }
    // And none on the freshness READER: `resolve_reader` must answer an
    // invisible id exactly as an unknown one, and write no cursor for
    // either (T7).
    for who in [Who::Stranger, Who::Master, Who::Legacy] {
        let hidden = call(
            &fx,
            who,
            "list_sessions",
            json!({ "summary": false, "fresh_for": fx.row }),
        )
        .await;
        let unknown = call(
            &fx,
            who,
            "list_sessions",
            json!({ "summary": false, "fresh_for": 999999 }),
        )
        .await;
        assert_eq!(
            text(&hidden),
            text(&unknown),
            "{who:?}: an invisible reader must read as an unknown one"
        );
    }
    // `broadcast_prompt` is RATE-LIMITED to one call per 30 s per caller
    // label, and two of the nine callers share a label (the two host tokens
    // on `h-row`) — so this row cannot share one fixture the way every other
    // row does. A fresh fixture per caller is cheap (an in-memory store) and
    // keeps the rule being measured intact.
    //
    // The rule is rule 1 and rule 3 on the FAN-OUT: `BroadcastFilter::view`
    // keeps only the rows this caller `may_drive`, so the tool refuses
    // nobody and simply has nothing to send — every caller below `drive` on
    // the private row broadcasts to an EMPTY set, and the answer names no
    // row.
    m.covered.insert("broadcast_prompt".to_string());
    for &who in EVERYONE {
        let own = fixture();
        let a = call(
            &own,
            who,
            "broadcast_prompt",
            json!({ "host": HOST, "project_id": null, "status": null, "prompt": "all hands" }),
        )
        .await;
        for mark in MARKERS {
            assert!(
                who.may_read_the_row() || !text(&a).contains(mark),
                "LEAK: {who:?} read {mark:?} through broadcast_prompt: {}",
                text(&a)
            );
        }
        assert!(
            !gate_refused(&a),
            "broadcast_prompt gates nobody: {who:?} got {a:?}"
        );
        // What must never happen is the PRIVATE row being in the fan-out.
        // An empty set is not the rule: §4.4 clause 1 lets a per-host token
        // drive the `unclaimed` rows on its own host, and the two host
        // callers duly reach those — a real difference the matrix records
        // rather than asserting away.
        if !matches!(who, Who::Owner | Who::Driver | Who::HostPane) {
            assert!(
                !mentions_session(text(&a), own.row),
                "{who:?} may not drive the private row, so the fan-out must \
                 not even attempt it: {a:?}"
            );
        }
    }
    m.row(
        "my_grants",
        |_, _| json!({}),
        |who| match who {
            // A per-host token proves no person, so it is refused outright.
            w if w.is_host() => Out::Code(codes::E_FORBIDDEN),
            _ => Out::Pass,
        },
    )
    .await;
    m.row("fleet_health", |_, _| json!({}), open).await;
    m.row("usage_report", |_, _| json!({}), open).await;
    // `list_downloads` is the INDEX into the bytes `send_file` copied — its
    // rows carry the file's absolute path on the owner's host — and it gates
    // no single row: it FILTERS, which is why `tests::NO_PER_ROW_GATE` holds
    // its reason and it has no `SESSION_REACH` row for `gated` to measure
    // against. That table's claim is "the page is cut by
    // `downloads::visible`, which asks `ViewScope::may_own` on the session
    // each row came out of — the same `own` tier `send_file` gates one row
    // with", and these cells plus the sweep below are the CHECKING of it: a
    // "it filters instead of gating" claim is exactly the kind a per-caller
    // row should test rather than accept.
    //
    // **The T8 result gate is no net underneath it.** A serialised
    // `DownloadRow` is `{id, at, host_alias, session_id, session_name, path,
    // …}`, and `looks_like_session_row` wants `visibility`, or `host_alias`
    // AND `tmux_name` — the download spells that key `session_name`, so the
    // gate does not recognise the shape and drops nothing. `visible` is the
    // whole of the fence, which is what makes this row load-bearing rather
    // than decorative.
    //
    // `session_id` is OPTIONAL, so both shapes run: the fleet-wide page, and
    // the session-addressed one naming a row the caller may not own — which
    // must yield an empty page and not a refusal, so it is no existence
    // oracle either. The three host callers never read either shape:
    // `list_downloads` is in `guard::NOT_FOR_HOST_TOKENS` (a host's Claude
    // only sends), recorded in `NEVER_A_HOST_TOKENS` above and refused by
    // `enforce_admin` before any row is read.
    //
    // What is deliberately NOT fenced: `total_bytes`, `max_total_bytes` and
    // `max_file_bytes`. Those are the MACHINE's budget, computed over every
    // row on purpose — a sender has to be told the disk is full by somebody
    // else's file — and they name no host, no session, no path and no person.
    let no_host_token = |who: Who| match who {
        w if w.is_host() => Out::Code(codes::E_FORBIDDEN),
        _ => Out::Pass,
    };
    m.row("list_downloads", |_, _| json!({}), no_host_token)
        .await;
    // `runs` (Orbit Fleet 8.3) FILTERS too (`tests::NO_PER_ROW_GATE`): its
    // union is cut in SQL by `service::runs::reach`, so a run in a session
    // this caller may not see is not listed, and naming that session finds
    // an empty page rather than a refusal. The leak check over both shapes is
    // the checking of that claim. Never a per-host token's.
    m.row("runs", |_, _| json!({ "action": "list" }), no_host_token)
        .await;
    m.row(
        "runs",
        |fx, _| json!({ "action": "list", "session_id": fx.row }),
        no_host_token,
    )
    .await;
    m.row(
        "list_downloads",
        |fx, _| json!({ "session_id": fx.row }),
        no_host_token,
    )
    .await;
    // The row above is held by `MARKERS` for the five callers that may not
    // read the session at all — but a WATCHER and a DRIVER may read it, so
    // the leak check is silent about precisely the two callers the `own` tier
    // exists to refuse. So the PAGE is asserted instead: only the owner's
    // `list_downloads` carries the file, in either shape. The owner's half is
    // not symmetry for its own sake — without it "the page is empty" would
    // pass for a fixture whose download nobody can see at all.
    for &who in EVERYONE {
        for args in [json!({}), json!({ "session_id": fx.row })] {
            let a = call(&fx, who, "list_downloads", args.clone()).await;
            if who.is_host() {
                assert_eq!(code(&a), codes::E_FORBIDDEN, "{who:?}: {a:?}");
                continue;
            }
            if who == Who::Owner {
                // Both halves: the ROW is there (by its own id, so a
                // same-shaped row of somebody else's cannot stand in for it)
                // and it carries the session's name — which is the marker
                // every other caller must not see.
                assert!(
                    text(&a).contains(&format!("\"id\":{}", fx.download))
                        && text(&a).contains(LEAK_TMUX),
                    "the owner's own file must still be served, or the \
                     emptiness below means nothing ({args}): {a:?}"
                );
                continue;
            }
            assert!(
                text(&a).contains("\"downloads\":[]"),
                "{who:?} does not own the session this file came out of, so \
                 the page must be EMPTY rather than merely redacted \
                 ({args}): {a:?}"
            );
        }
    }
    {
        // And the budget really is served to everyone, so the emptiness above
        // is the ROWS being cut and not the whole answer being withheld.
        let a = call(&fx, Who::Stranger, "list_downloads", json!({})).await;
        assert!(
            text(&a).contains("\"total_bytes\":11"),
            "the machine's budget is nobody's secret: {a:?}"
        );
    }
    // Control's Library (9.7): the same `own` tier as `list_downloads`, for
    // the same reason (its rows name paths on the owner's host). `list` is a
    // filter, so a session the caller does not own answers an empty page;
    // `add` names one row, so it refuses everybody but the owner with
    // `E_NOTFOUND`, the answer an id that is not theirs gets.
    m.row("library", |_, _| json!({ "action": "list" }), no_host_token)
        .await;
    m.row(
        "library",
        |fx, _| json!({ "action": "list", "session_id": fx.row }),
        no_host_token,
    )
    .await;
    for &who in EVERYONE {
        for args in [
            json!({ "action": "list" }),
            json!({ "action": "list", "session_id": fx.row }),
        ] {
            let a = call(&fx, who, "library", args.clone()).await;
            if who.is_host() {
                assert_eq!(code(&a), codes::E_FORBIDDEN, "{who:?}: {a:?}");
                continue;
            }
            if who == Who::Owner {
                assert!(
                    text(&a).contains(&format!("\"id\":{}", fx.library_item))
                        && text(&a).contains(LEAK_TMUX),
                    "the owner's own file must still be listed ({args}): {a:?}"
                );
                continue;
            }
            assert!(
                text(&a).contains("\"items\":[]"),
                "{who:?} does not own the session, so the page must be EMPTY \
                 ({args}): {a:?}"
            );
        }
        let add = json!({
            "action": "add",
            "kind": "upload",
            "session_id": fx.row,
            "files": [{ "path": "/src/acme/wt/x.txt" }],
        });
        let a = call(&fx, who, "library", add).await;
        match who {
            w if w.is_host() => assert_eq!(code(&a), codes::E_FORBIDDEN, "{who:?}: {a:?}"),
            Who::Owner => assert_eq!(code(&a), "OK", "the owner records its own file: {a:?}"),
            _ => assert_eq!(
                code(&a),
                codes::E_NOTFOUND,
                "{who:?} adds beside a session it does not own: {a:?}"
            ),
        }
    }
    m.row(
        "list_worktrees",
        |fx, _| json!({ "project_id": fx.project, "summary": false }),
        open,
    )
    .await;
    m.row(
        "list_host_worktrees",
        |fx, _| json!({ "host_alias": HOST, "project_id": fx.project }),
        |who| match who {
            Who::HostElsewhere => Out::Code(codes::E_FORBIDDEN),
            _ => Out::Pass,
        },
    )
    .await;
    m.row(
        "discover_lost_sessions",
        |_, _| json!({ "host_alias": HOST }),
        |who| match who {
            Who::HostElsewhere => Out::Code(codes::E_FORBIDDEN),
            _ => Out::Pass,
        },
    )
    .await;
    // A found conversation nobody is recorded against: the host fence
    // refuses a token bound elsewhere, and the person fence passes it.
    m.row(
        "place_transcript",
        |fx, _| {
            json!({
                "host_alias": HOST,
                "claude_session_id": "44366faf-ae97-426a-91cd-beaf3c74f1d7",
                "project_id": fx.project,
            })
        },
        |who| match who {
            Who::HostElsewhere => Out::Code(codes::E_FORBIDDEN),
            _ => Out::Pass,
        },
    )
    .await;
    // Served to a peer-mode token and to nothing else.
    m.row(
        "peer_exchange",
        |_, _| json!({ "proto": 1, "fleet_id": "peer-fleet" }),
        |_| Out::Code(codes::E_FORBIDDEN),
    )
    .await;

    // `ask` reaches a session through the form's row, not a `session_id`
    // argument: `get` reads it, `decline` (a person's call; a host token is
    // refused before the gate) drives it. One pending form on the private
    // row serves both, `get` first because `decline` finishes it.
    let form_id = crate::service::forms::open(
        &fx.t.store,
        fx.row,
        &json!({ "spec": "fleet.form/1", "title": "Pick", "steps": [
            { "title": "One", "fields": [ { "name": "x", "type": "text", "label": "X" } ] } ] }),
        None,
    )
    .expect("a form opens on the private row")
    .form_id;
    m.gated("ask", Reach::Read, |_, _| json!({ "get": form_id }))
        .await;
    m.at(
        "ask",
        Reach::Drive,
        |_, _| json!({ "decline": form_id }),
        |who| {
            if who.is_host() {
                Out::Code(codes::E_FORBIDDEN)
            } else {
                tier(Reach::Drive, who)
            }
        },
    )
    .await;

    // ---- coverage --------------------------------------------------------
    let want = want_covered();
    let missing: Vec<&String> = want.iter().filter(|t| !m.covered.contains(*t)).collect();
    assert!(
        missing.is_empty(),
        "session-addressed tools without a matrix row: {missing:?} — add a \
         `Matrix::row` call and a `call` arm, or say in \
         SESSION_REACHABLE_WITHOUT_A_KEY why the tool reaches no session"
    );
}

/// The covered set, derived: every tool `tests::session_addressed_tools()`
/// sees, plus the session-REACHABLE surfaces whose schema names no session
/// ([`SESSION_REACHABLE_WITHOUT_A_KEY`]).
fn want_covered() -> BTreeSet<String> {
    let mut want: BTreeSet<String> = super::tests::session_addressed_tools()
        .into_keys()
        .collect();
    want.extend(
        SESSION_REACHABLE_WITHOUT_A_KEY
            .iter()
            .map(|(n, _)| (*n).to_string()),
    );
    want
}

#[tokio::test]
async fn the_session_matrix_holds_for_every_session_addressed_tool() {
    run_matrix().await;
}

/// The coverage half, as a test of its own so that a tool added with no row
/// fails here even when the behavioural run is filtered out.
///
/// It also keeps the derivation honest in the other direction: every name in
/// [`SESSION_REACHABLE_WITHOUT_A_KEY`] must be a tool the router really
/// serves AND one the schema derivation does NOT already see — a row that
/// has become redundant is a row nobody rechecks.
#[test]
fn every_session_addressed_tool_has_a_matrix_row() {
    let served: BTreeSet<String> = FleetTools::tool_router_for_doc()
        .list_all()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    let addressed = super::tests::session_addressed_tools();
    for (name, why) in SESSION_REACHABLE_WITHOUT_A_KEY {
        assert!(
            served.contains(*name),
            "{name} is in this file's extension list but the router serves no \
             such tool ({why})"
        );
        assert!(
            !addressed.contains_key(*name),
            "{name} now names a session in its schema, so \
             `session_addressed_tools` already covers it: drop the extension \
             row ({why})"
        );
    }
    // The derivation's own health is `tests.rs`'s to assert (sentinels and a
    // floor); what this says is that the authority answered a set at all.
    assert!(
        addressed.len() >= 45,
        "only {} session-addressed tools were derived: the authority's \
         predicate has collapsed, and every row below it covers nothing",
        addressed.len()
    );
    // The matrix's own arms, read off the harness rather than re-listed:
    // a tool in the covered set with no arm panics inside `call`, so the
    // behavioural test above is what enforces that half. Here we only hold
    // the two sets to each other.
    let want = want_covered();
    for name in &want {
        assert!(
            served.contains(name),
            "{name} is in the covered set but the router serves no such tool"
        );
    }
}

/// **The Answer level (Orbit Fleet 11.7), as its own row.** `send_prompt`
/// is one tool at two tiers: a prompt is `drive`, a key alone is `answer`.
/// The matrix row above pins the prompt; this pins the key, for every
/// caller, plus the two refusals only an answer grant can meet — `C-c`, and
/// a prompt.
#[tokio::test]
async fn an_answer_grant_presses_a_key_and_nothing_wider() {
    let fx = fixture();
    let key = |k: &str| json!({ "session_id": fx.row, "prompt": "", "keys": k });
    for who in EVERYONE.iter().copied() {
        let a = call(&fx, who, "send_prompt", key("1")).await;
        let want = tier(Reach::Answer, who);
        match want {
            // The gate let it through; the fixture host has no SSH, so the
            // pane read (or the press) fails for a reason that is not access.
            Out::Pass => assert!(!gate_refused(&a), "{who:?} answers: {a:?}"),
            Out::Gate(c) => {
                assert_eq!(code(&a), c, "{who:?}: {a:?}");
                assert!(gate_refused(&a), "{who:?} is refused by the gate: {a:?}");
            }
            Out::GateAny(cs) => {
                assert!(cs.contains(&code(&a)), "{who:?}: {a:?}");
                assert!(gate_refused(&a), "{who:?} is refused by the gate: {a:?}");
            }
            Out::Code(c) => assert_eq!(code(&a), c, "{who:?}: {a:?}"),
        }
    }
    // A watcher is told the level it lacks, and it is `answer`.
    let w = call(&fx, Who::Watcher, "send_prompt", key("1")).await;
    assert!(text(&w).contains("answer"), "{w:?}");
    // C-c interrupts rather than answers: refused before the pane is read.
    let c = call(&fx, Who::Answerer, "send_prompt", key("C-c")).await;
    assert_eq!(code(&c), codes::E_FORBIDDEN, "{c:?}");
    assert!(text(&c).contains("not C-c"), "{c:?}");
    // The driver keeps C-c: its tier is `drive`, not the answer rule.
    let d = call(&fx, Who::Driver, "send_prompt", key("C-c")).await;
    assert!(!text(&d).contains("not C-c"), "{d:?}");
    // And a prompt is `drive`, which an answer grant is not.
    let p = call(
        &fx,
        Who::Answerer,
        "send_prompt",
        json!({ "session_id": fx.row, "prompt": "go" }),
    )
    .await;
    assert_eq!(code(&p), codes::E_FORBIDDEN, "{p:?}");
    assert!(text(&p).contains("drive"), "{p:?}");
}

/// The two refusals are different on purpose, and this is the pin: a WATCHER
/// is told the level it lacks, a STRANGER is told the row does not exist —
/// and the stranger's answer is byte-identical to the one an id that never
/// existed produces, so no refusal is an existence oracle.
#[tokio::test]
async fn a_watcher_and_a_stranger_are_told_apart_only_by_e_forbidden_vs_e_notfound() {
    let fx = fixture();
    // One `Drive` tool and one `Own` tool: the watcher lacks both levels,
    // the stranger lacks the row.
    {
        let args = json!({ "session_id": fx.row, "prompt": "go" });
        let w = call(&fx, Who::Watcher, "send_prompt", args.clone()).await;
        assert_eq!(code(&w), codes::E_FORBIDDEN, "{w:?}");
        assert!(
            text(&w).contains("drive"),
            "a watcher is told which level it lacks: {w:?}"
        );
        let s = call(&fx, Who::Stranger, "send_prompt", args).await;
        assert_eq!(code(&s), codes::E_NOTFOUND, "{s:?}");
        let unknown = call(
            &fx,
            Who::Stranger,
            "send_prompt",
            json!({ "session_id": 999999, "prompt": "go" }),
        )
        .await;
        same_as_unknown(&s, &unknown, &fx.row.to_string(), "999999");
    }
    // And the same for a read, where the watcher is the one who gets in.
    let w = call(
        &fx,
        Who::Watcher,
        "session_history",
        json!({ "session_id": fx.row }),
    )
    .await;
    assert!(!gate_refused(&w), "a watcher reads: {w:?}");
    let s = call(
        &fx,
        Who::Stranger,
        "session_history",
        json!({ "session_id": fx.row }),
    )
    .await;
    assert_eq!(code(&s), codes::E_NOTFOUND, "{s:?}");
    let unknown = call(
        &fx,
        Who::Stranger,
        "session_history",
        json!({ "session_id": 999999 }),
    )
    .await;
    same_as_unknown(&s, &unknown, &fx.row.to_string(), "999999");
}

/// Spec §4.3 invariant 5, as the one list T7 cites and this test drives: a
/// `drive` grantee is refused **every** tool in the `own` tier, and the
/// refusal is normally a LEVEL (`E_FORBIDDEN`) rather than a missing row —
/// the driver can see the session, it simply may not dispose of it. Two tools
/// are STRICTER than that and say so in the `want` list below
/// (`work_link`'s conversation arms, and `send_file`, whose gate has one
/// refusing arm): being told less than the tier would tell you is never the
/// failure this test is looking for.
///
/// The list is read off `SESSION_REACH`, not retyped: a tool whose row moves
/// to `Own` joins this test automatically, which is the only way a tier
/// list written today still means something next year.
#[tokio::test]
async fn a_driver_is_refused_every_owner_only_tool() {
    let fx = fixture();
    let own_tier: Vec<&str> = super::tests::SESSION_REACH
        .iter()
        .filter(|(_, r)| r.contains(&"Own"))
        .map(|(n, _)| *n)
        .collect();
    assert!(
        own_tier.len() >= 15,
        "the `own` tier has collapsed to {own_tier:?}"
    );
    // One call per `own`-tier tool, with the arguments the matrix uses.
    for tool in &own_tier {
        let args = own_tier_args(&fx, tool);
        let a = call(&fx, Who::Driver, tool, args).await;
        // Four shapes of "no", and each one is the tool's own design:
        //
        // * `restore_host_sessions` is the BATCH, addressed by host: it never
        //   refuses, it reports the row as a `skip` reading `not found on
        //   this host`. The refusal is still total — nothing is restarted —
        //   and the report names no session content;
        // * `work_link`'s `own` arms are addressed by a work key and a
        //   `link_id`, so the refusal is `summary::no_such_link`: the driver
        //   reads as if the link were not there rather than being told a tier
        //   it lacks;
        // * `send_file` is addressed by `session_id` like most of the tier,
        //   but its gate lives in `service::downloads::send` rather than in
        //   `resolve_row_and_gate`, and that gate has a single refusing arm:
        //   the driver reads as if the session were not there. Stricter than
        //   the tier, since a download is an unconstrained read of the
        //   owner's host;
        // * everything else names the tier, which reveals nothing the driver
        //   could not already see.
        if *tool == "restore_host_sessions" {
            assert!(
                text(&a).contains(crate::service::sessions::NOT_ON_THIS_HOST)
                    && text(&a).contains("\"skip\""),
                "a `drive` grantee's batch restore must SKIP the owner's row: {a:?}"
            );
            continue;
        }
        let want: &[&str] = match *tool {
            "work_link" => &[codes::E_FORBIDDEN, codes::E_NOTFOUND],
            // `send_file` is the fourth shape, and the STRICTEST: its gate
            // (`downloads::send`) has one `_ =>` arm answering
            // `session {id} not found`, so the driver — who can see the
            // session perfectly well — is told the row does not exist. More
            // than §4.3 asks for, recorded rather than smoothed; the matrix
            // row above pins the whole column and a no-oracle sweep next to
            // it pins the sentence.
            "send_file" => &[codes::E_NOTFOUND],
            _ => &[codes::E_FORBIDDEN],
        };
        assert!(
            want.contains(&code(&a)),
            "a `drive` grantee must be refused {tool} at the OWN tier \
             (expected one of {want:?}): {a:?}"
        );
    }
}

/// The arguments the `own`-tier sweep calls each tool with. A `match` with
/// no catch-all, so a tool that joins the tier makes this fail to compile
/// its way out of being tested — the same discipline as `call`'s dispatch.
fn own_tier_args(fx: &Fx, tool: &str) -> Value {
    match tool {
        "kill_session" | "safe_kill_session" | "restart_session" | "recreate_session"
        | "session_access" | "shell_terminals" => json!({ "session_id": fx.row }),
        "access_requests" => json!({ "action": "list", "session_id": fx.row }),
        "move_session" => {
            json!({ "session_id": fx.row, "target_host_alias": FAR, "dry_run": true })
        }
        "resolve_move" => json!({ "session_id": fx.row, "action": "finish" }),
        "spawn_review" => json!({ "source_session_id": fx.row, "prompt": "review it" }),
        "rewind_conversation" => json!({ "session_id": fx.row, "mode": "fork" }),
        "restore_host_sessions" => {
            json!({ "host_alias": HOST, "session_ids": [fx.row], "dry_run": true })
        }
        "rename_session" => json!({ "session_id": fx.row, "new_name": "renamed" }),
        "adopt_session" | "lost_target" => json!({ "session_id": fx.row }),
        "set_session_tags" => json!({ "session_id": fx.row, "tags": ["t"] }),
        "decide_related_session" => json!({ "session_id": fx.row, "run_id": 1, "linked": true }),
        "delete_worktree" => json!({ "worktree_id": fx.worktree, "force": true }),
        "session_share" | "session_unshare" | "session_narrow" => {
            json!({ "session_id": fx.row, "person": PERSON_SPARE, "level": "watch" })
        }
        "work_link" => {
            json!({ "action": "summarize", "key": WORK_KEY, "link_id": fx.link })
        }
        // A RELATIVE path, which is the shape the tool is documented with and
        // the only one that could plausibly be innocent: the point is that
        // the driver is refused before `stat_script` is ever built, so which
        // file was asked for never matters.
        "send_file" => json!({ "session_id": fx.row, "path": "out/report.pdf" }),
        other => panic!("no `own`-tier arguments for {other}"),
    }
}

/// Rule 6, first half: **claiming needs proof of HOST ACCESS to that row's
/// pane, not of the host.** A per-host token standing in an unclaimed row's
/// pane claims that row; the SAME token cannot claim the unclaimed row next
/// to it, whose pane it does not prove.
///
/// This is the pin against the defect class the milestone keeps producing —
/// a session resolved by something reusable. Being on the host is reusable
/// (one token authenticates every Claude on it); a pane id the row itself
/// carries is not.
#[tokio::test]
async fn claiming_needs_the_pane_of_the_row_not_merely_its_host() {
    let fx = fixture();
    let in_pane = Caller {
        api: None,
        host_alias: Some(HOST.into()),
        client: None,
        mode: TokenMode::Full,
        pane: Some(PANE_UNCLAIMED.into()),
        is_personal_owner: false,
    };
    // The sibling row, on the same host, with the same token: refused.
    let other =
        fx.t.session_claim(
            Extension(in_pane.clone()),
            Parameters(
                serde_json::from_value(
                    json!({ "session_id": fx.unclaimed_other, "person": PERSON_OWNER }),
                )
                .unwrap(),
            ),
        )
        .await;
    let other = match other {
        Ok(r) => {
            let body: String = r
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect();
            if r.is_error == Some(true) {
                Err(body)
            } else {
                Ok(body)
            }
        }
        Err(e) => Err(e.message.to_string()),
    };
    assert_eq!(
        code(&other),
        codes::E_FORBIDDEN,
        "being on the host is not enough to claim a row whose pane this \
         request does not prove: {other:?}"
    );
    {
        let s = fx.t.store.lock().unwrap();
        assert_eq!(
            s.get_session_by_id(fx.unclaimed_other)
                .unwrap()
                .unwrap()
                .owner_person_id,
            None,
            "the refused claim wrote nothing"
        );
    }
    // Its own row, whose pane it proves: claimed, and private to the person.
    let mine =
        fx.t.session_claim(
            Extension(in_pane),
            Parameters(
                serde_json::from_value(
                    json!({ "session_id": fx.unclaimed, "person": PERSON_OWNER }),
                )
                .unwrap(),
            ),
        )
        .await
        .expect("the pane's own row claims");
    assert_ne!(mine.is_error, Some(true), "{mine:?}");
    let s = fx.t.store.lock().unwrap();
    let got = s.get_session_by_id(fx.unclaimed).unwrap().unwrap();
    assert_eq!(got.owner_person_id, Some(fx.owner));
    assert_eq!(
        got.visibility,
        crate::store::VISIBILITY_PRIVATE,
        "a claimed row is private, not left open"
    );
}

/// Rule 6, second half: **an unclaimed session leaks no metadata.** Not its
/// name, not its host's page of rows — a per-host count is the whole of what
/// §4.3 allows, and that count is `HostRow.unclaimed_sessions`, served to
/// one person or to nobody.
///
/// Five people live on this hub, so `sole_enabled_person` answers `None` and
/// the single-person carve-out is off: no person caller here may see the row
/// at all.
#[tokio::test]
async fn an_unclaimed_row_leaks_no_metadata_to_a_person() {
    let fx = fixture();
    for who in [
        Who::Owner,
        Who::Watcher,
        Who::Driver,
        Who::Stranger,
        Who::Master,
        Who::Legacy,
    ] {
        for (tool, args) in [
            ("list_sessions", json!({ "summary": false })),
            ("list_sessions", json!({ "summary": true })),
        ] {
            let a = call(&fx, who, tool, args).await;
            assert!(
                !text(&a).contains(UNCLAIMED_NAME),
                "{who:?} read an unclaimed row's name through {tool}: {}",
                text(&a)
            );
        }
        // By NAME it is a miss, and the same miss a name nobody used gives.
        // The refusal quotes the name the caller typed, which is its own
        // input and no leak, so the pin here is the no-oracle one.
        let hidden = call(&fx, who, "whoami", json!({ "tmux_name": UNCLAIMED_NAME })).await;
        let unknown = call(&fx, who, "whoami", json!({ "tmux_name": "no-such-name" })).await;
        assert_eq!(code(&hidden), codes::E_NOTFOUND, "{who:?}: {hidden:?}");
        same_as_unknown(&hidden, &unknown, UNCLAIMED_NAME, "no-such-name");
        // Reading it by id is a miss, and the same miss an id that does not
        // exist produces.
        let hidden = call(
            &fx,
            who,
            "session_history",
            json!({ "session_id": fx.unclaimed }),
        )
        .await;
        let unknown = call(&fx, who, "session_history", json!({ "session_id": 999999 })).await;
        assert_eq!(code(&hidden), codes::E_NOTFOUND, "{who:?}: {hidden:?}");
        same_as_unknown(&hidden, &unknown, &fx.unclaimed.to_string(), "999999");
    }
}

/// The two pre-gate policy fences this file's cells rely on, asserted
/// against the policy tables themselves rather than against behaviour: a
/// cell that predicted `E_FORBIDDEN` because a tool is "never a host
/// token's" is only as true as that membership.
#[test]
fn the_pre_gate_policy_fences_are_what_the_cells_assume() {
    for t in NEVER_A_HOST_TOKENS {
        assert!(
            guard::NOT_FOR_HOST_TOKENS.contains(t),
            "{t} is not in NOT_FOR_HOST_TOKENS, so this matrix's host cells \
             are predicting a refusal nothing produces"
        );
    }
    for t in ONLY_A_HOST_TOKENS {
        assert_eq!(
            guard::policy(t).map(|p| p.access),
            Some(guard::Access::HostToken),
            "{t} is not Access::HostToken any more"
        );
    }
    for t in ONLY_A_PEER {
        assert!(
            guard::policy(t).is_some(),
            "{t} has no ToolPolicy, so nothing says who may call it"
        );
    }
}

/// A map from the file's own tables, so the reasons stay machine-checked
/// rather than decorative.
#[test]
fn every_extension_row_has_a_reason() {
    let reasons: BTreeMap<&str, &str> = SESSION_REACHABLE_WITHOUT_A_KEY.iter().copied().collect();
    assert_eq!(
        reasons.len(),
        SESSION_REACHABLE_WITHOUT_A_KEY.len(),
        "a duplicate row in SESSION_REACHABLE_WITHOUT_A_KEY"
    );
    for (name, why) in &reasons {
        assert!(
            why.len() > 30,
            "{name}'s reason is too short to be a claim a reviewer can check"
        );
    }
}
