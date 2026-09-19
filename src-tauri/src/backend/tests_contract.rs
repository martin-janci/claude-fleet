//! The field-name contract tests. See [`super`] for why they exist.

use super::*;
use fleet_core::service::health::Health;
use fleet_core::service::projects::ProjectTreeRow;
use fleet_core::service::repo_read::{
    Branch, ChangedFile, Commit, CommitDetail, FileContent, FileDiff, GitRef, RepoTree,
};
use fleet_core::service::transcript::{ConvItem, ConvTurn, Conversation};
use fleet_core::service::usage::DayUsage;
use fleet_core::service::worktrees::{WorktreeOccupancy, WorktreeOccupant};
use fleet_core::store::{
    AccountRow, HostRow, ProjectRow, SessionEvent, SessionRow, SessionUsage, TaskRow, UsageTotals,
    WorktreeRow,
};
use std::collections::BTreeMap;

// ── fully populated samples ─────────────────────────────────────────────────
//
// Every `Option` is `Some` and every `Vec`/map is non-empty, so no key can be
// missing from the serialised form for want of a value. A field that
// `skip_serializing_if`s itself away when empty would otherwise slip out of
// the contract unnoticed.

fn sample_usage() -> SessionUsage {
    SessionUsage {
        usage_input_tokens: 11,
        usage_output_tokens: 12,
        usage_cache_write_tokens: 13,
        usage_cache_read_tokens: 14,
        usage_cost_micros: 15,
        usage_model: Some("claude-opus-5".into()),
        usage_updated_at: Some(1_726_000_000),
    }
}

pub(crate) fn sample_session() -> SessionRow {
    SessionRow {
        id: 1,
        tmux_name: "fleet-demo".into(),
        host_alias: "trn".into(),
        project_id: Some(2),
        worktree_id: Some(3),
        created_at: 1_725_000_000,
        last_activity_at: 1_725_000_100,
        status: "alive".into(),
        notes: Some("a note".into()),
        account_uuid: Some("acct-uuid".into()),
        kind: "work".into(),
        reviews_session_id: Some(4),
        worktree_key: Some("owner/repo:branch".into()),
        lost_at: Some(1_725_000_200),
        claude_session_id: Some("claude-uuid".into()),
        claude_status: Some("working".into()),
        effort_level: Some("high".into()),
        pr_url: Some("https://github.com/o/r/pull/1".into()),
        current_activity: Some("Reading files".into()),
        context_pct: Some(91.5),
        stuck_kind: Some("auth_menu".into()),
        friendly_name: Some("the demo".into()),
        safe_kill_state: Some("asked".into()),
        safe_kill_nonce: Some("nonce".into()),
        safe_kill_detail: Some("detail".into()),
        safe_kill_requested_at: Some(1_725_000_300),
        idle_since: Some(1_725_000_400),
        stuck_since: Some(1_725_000_500),
        last_playbook_at: Some(1_725_000_600),
        last_prompt: Some("do the thing".into()),
        started_at: Some(1_725_000_700),
        last_turn_at: Some(1_725_000_800),
        ci_status: Some("passing".into()),
        turn_seq: 7,
        last_stop_at: Some(1_725_000_900),
        parent_session_id: Some(5),
        tags: vec!["tag-a".into(), "tag-b".into()],
        usage: sample_usage(),
    }
}

pub(crate) fn sample_host() -> HostRow {
    HostRow {
        alias: "trn".into(),
        ssh_alias: Some("trn.example".into()),
        reachable: true,
        claude_version: Some("2.0.0".into()),
        tmux_version: Some("3.4".into()),
        hidden: false,
        last_pinged_at: Some(1_725_000_000),
        account_uuid: Some("acct-uuid".into()),
        provisioned: true,
        // Consistent with the `ssh_alias` above: this sample is an SSH host.
        // `transport` is not an `Option`, so either value pins the same key.
        transport: "ssh".into(),
    }
}

pub(crate) fn sample_account() -> AccountRow {
    AccountRow {
        uuid: "acct-uuid".into(),
        email: Some("a@example.com".into()),
        display_name: Some("A Person".into()),
        organization_name: Some("Org".into()),
        organization_uuid: Some("org-uuid".into()),
        seat_tier: Some("max".into()),
        last_seen_at: Some(1_725_000_000),
        nickname: Some("work".into()),
        has_extra_usage: true,
    }
}

fn sample_event() -> SessionEvent {
    SessionEvent {
        id: 1,
        session_id: 2,
        at: 1_725_000_000,
        kind: "prompt_sent".into(),
        detail: Some("detail".into()),
    }
}

pub(crate) fn sample_task() -> TaskRow {
    TaskRow {
        id: 1,
        requester_session_id: Some(2),
        worker_session_id: Some(3),
        prompt: Some("do it".into()),
        state: "running".into(),
        result: Some("done".into()),
        error: Some("nope".into()),
        created_at: 1_725_000_000,
        started_at: Some(1_725_000_100),
        finished_at: Some(1_725_000_200),
        nonce: "secret-nonce".into(),
        worker_claude_session_id: Some("claude-uuid".into()),
    }
}

pub(crate) fn sample_project_row() -> ProjectRow {
    ProjectRow {
        id: 1,
        owner: "owner".into(),
        repo: "repo".into(),
        base_path: "/home/dev/projects".into(),
        last_session_at: Some(1_725_000_000),
        adopted: true,
    }
}

pub(crate) fn sample_worktree_row() -> WorktreeRow {
    WorktreeRow {
        id: 1,
        project_id: 2,
        host_alias: "trn".into(),
        name: "feat-x".into(),
        path: "/home/dev/projects/.worktrees/feat-x".into(),
        branch: Some("feat/x".into()),
    }
}

fn sample_project_tree() -> ProjectTreeRow {
    ProjectTreeRow {
        project: sample_project_row(),
        worktrees: vec![sample_worktree_row()],
    }
}

fn sample_occupancy() -> WorktreeOccupancy {
    WorktreeOccupancy {
        worktree: sample_worktree_row(),
        occupants: vec![WorktreeOccupant {
            host_alias: "trn".into(),
            tmux_name: "fleet-demo".into(),
        }],
    }
}

fn sample_totals() -> UsageTotals {
    UsageTotals {
        input_tokens: 1,
        output_tokens: 2,
        cache_write_tokens: 3,
        cache_read_tokens: 4,
        cost_micros: 5,
    }
}

fn sample_health() -> Health {
    Health {
        version: "0.2.20".into(),
        db_ready: true,
        schema_version: 42,
        hosts_reachable: 2,
        hosts_total: 3,
        sessions_total: 4,
        by_status: BTreeMap::from([("working".to_string(), 1u32)]),
        ghosts: 1,
        context_red: 1,
        stuck: 1,
        usage_by_host: BTreeMap::from([("trn".to_string(), sample_totals())]),
        usage_by_day: vec![DayUsage {
            day: "2026-09-18".into(),
            totals: sample_totals(),
        }],
    }
}

fn sample_conversation() -> Conversation {
    Conversation {
        turns: vec![ConvTurn {
            prompt: Some("hello".into()),
            at: Some("2026-09-18T10:00:00Z".into()),
            ended_at: Some("2026-09-18T10:00:05Z".into()),
            items: vec![
                ConvItem::Text {
                    text: "hi back".into(),
                },
                ConvItem::Tool {
                    summary: "Read(src/lib.rs)".into(),
                    error: true,
                },
            ],
        }],
        truncated: true,
    }
}

fn sample_changed_file() -> ChangedFile {
    ChangedFile {
        path: "src/lib.rs".into(),
        status: "renamed".into(),
        staged: true,
        orig_path: Some("src/old.rs".into()),
    }
}

fn sample_git_ref() -> GitRef {
    GitRef {
        name: "main".into(),
        kind: "branch".into(),
    }
}

// ── the contract ────────────────────────────────────────────────────────────

/// Every type the desktop deserialises out of a hub answer, with the keys it
/// actually puts on the wire. The names on the right are the contract.
fn the_whole_contract() -> BTreeMap<String, Vec<String>> {
    let mut c: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut put = |name: &str, keys: Vec<String>| {
        c.insert(name.to_string(), keys);
    };
    put("SessionRow", wire_keys(&sample_session()));
    put("HostRow", wire_keys(&sample_host()));
    put("AccountRow", wire_keys(&sample_account()));
    put("SessionEvent", wire_keys(&sample_event()));
    put("TaskRow", wire_keys(&sample_task()));
    put("ProjectTreeRow", wire_keys(&sample_project_tree()));
    put("ProjectRow", wire_keys(&sample_project_row()));
    put("WorktreeRow", wire_keys(&sample_worktree_row()));
    put("WorktreeOccupancy", wire_keys(&sample_occupancy()));
    put(
        "WorktreeOccupant",
        wire_keys(&WorktreeOccupant {
            host_alias: "trn".into(),
            tmux_name: "fleet-demo".into(),
        }),
    );
    put("Health", wire_keys(&sample_health()));
    put("UsageTotals", wire_keys(&sample_totals()));
    put(
        "DayUsage",
        wire_keys(&DayUsage {
            day: "2026-09-18".into(),
            totals: sample_totals(),
        }),
    );
    put("Conversation", wire_keys(&sample_conversation()));
    put(
        "ConvTurn",
        wire_keys(&sample_conversation().turns.into_iter().next().unwrap()),
    );
    put(
        "ConvItem::Text",
        wire_keys(&ConvItem::Text { text: "t".into() }),
    );
    put(
        "ConvItem::Tool",
        wire_keys(&ConvItem::Tool {
            summary: "s".into(),
            error: false,
        }),
    );
    // The eight repo-browsing reads.
    put("ChangedFile", wire_keys(&sample_changed_file()));
    put(
        "RepoTree",
        wire_keys(&RepoTree {
            entries: vec!["src/lib.rs".into()],
            truncated: true,
        }),
    );
    put(
        "FileContent",
        wire_keys(&FileContent {
            path: "src/lib.rs".into(),
            content: "fn main() {}".into(),
            truncated: true,
            binary: false,
            is_dir: false,
            size: Some(12),
        }),
    );
    put(
        "FileDiff",
        wire_keys(&FileDiff {
            path: "src/lib.rs".into(),
            diff: "@@".into(),
            binary: false,
            truncated: true,
        }),
    );
    put(
        "Branch",
        wire_keys(&Branch {
            name: "main".into(),
            is_current: true,
            is_remote: false,
            upstream: Some("origin/main".into()),
            ahead: 1,
            behind: 2,
            tip_hash: "abc123".into(),
        }),
    );
    put("GitRef", wire_keys(&sample_git_ref()));
    put(
        "Commit",
        wire_keys(&Commit {
            hash: "abc123".into(),
            short_hash: "abc".into(),
            parents: vec!["def456".into()],
            refs: vec![sample_git_ref()],
            author: "A Person".into(),
            date: "2026-09-18".into(),
            subject: "do the thing".into(),
        }),
    );
    put(
        "CommitDetail",
        wire_keys(&CommitDetail {
            hash: "abc123".into(),
            subject: "do the thing".into(),
            body: "body".into(),
            author: "A Person".into(),
            date: "2026-09-18".into(),
            files: vec![sample_changed_file()],
        }),
    );
    c
}

/// The golden file's shape: the wire-key contract plus the wire-contract
/// revision it was generated at. The revision ties this file to
/// [`fleet_core::wire_contract::CONTRACT_REVISION`] — see
/// `the_goldens_revision_matches_the_wire_contract_constant` and the
/// REGEN branch of the test below.
#[derive(serde::Serialize, serde::Deserialize)]
struct Golden {
    revision: u32,
    types: BTreeMap<String, Vec<String>>,
}

/// The golden file, as committed.
fn golden_on_disk() -> Golden {
    let raw = include_str!("hub_contract.golden.json");
    serde_json::from_str(raw).expect("the golden file must be {revision, types: name -> [keys]}")
}

/// Absolute path to the golden, for the regenerate path. `CARGO_MANIFEST_DIR`
/// is `src-tauri/`.
fn golden_abs() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN_PATH)
}

/// **The contract test.** Every field name the desktop deserialises, pinned.
///
/// The names live in a committed file rather than in this function's body, so
/// a rename shows up as a reviewable diff instead of as a wall of literals.
/// The three fields where a wrong default is *invisible* are additionally
/// asserted as literals below, because those must not be regeneratable
/// without someone reading the line.
#[test]
fn the_hubs_field_names_are_the_ones_the_desktop_reads() {
    let actual = the_whole_contract();
    if std::env::var(REGEN_ENV).is_ok() {
        let old = golden_on_disk();
        // The one thing worth checking automatically before rewriting: did
        // this regeneration rename or drop a field (the dangerous case)
        // without CONTRACT_REVISION moving? An additive regen — a brand new
        // type, an extra key — is fine at the same revision; this only
        // fires for the change the whole module exists to catch.
        let lost = types_that_lost_fields(&old.types, &actual);
        let revision_bumped = fleet_core::wire_contract::CONTRACT_REVISION > old.revision;
        let golden = Golden {
            revision: fleet_core::wire_contract::CONTRACT_REVISION,
            types: actual.clone(),
        };
        let mut json = serde_json::to_string_pretty(&golden).unwrap();
        json.push('\n');
        std::fs::write(golden_abs(), json).expect("write the golden");
        if !lost.is_empty() && !revision_bumped {
            panic!(
                "{GOLDEN_PATH} was regenerated with a field renamed or removed, \
                 but fleet_core::wire_contract::CONTRACT_REVISION is still {}:\n\n{}\n\n\
                 Bump CONTRACT_REVISION in crates/fleet-core/src/wire_contract.rs \
                 before regenerating, so a hub still shaped like the old \
                 contract is refused by an updated desktop rather than \
                 silently trusted.",
                old.revision,
                lost.join("\n"),
            );
        }
        // Deliberately fails after rewriting. Regenerating this file is never
        // the end of the job — someone has to read the diff and decide
        // whether the hub renaming that field was meant to happen. A green
        // run would let it pass unread, which is the failure mode the whole
        // module exists to close.
        panic!(
            "{GOLDEN_PATH} was regenerated. Read `git diff -- {GOLDEN_PATH}`: \
             a key that changed name is a field the desktop will silently \
             default from here on. Then unset {REGEN_ENV} and run again."
        );
    }
    let golden = golden_on_disk();
    assert_eq!(
        golden.revision,
        fleet_core::wire_contract::CONTRACT_REVISION,
        "{GOLDEN_PATH} was generated at wire-contract revision {}, but \
         fleet_core::wire_contract::CONTRACT_REVISION is now {} — regenerate \
         with `{REGEN_ENV}=1 cargo test -p claude-fleet --lib contract` so \
         the golden's recorded revision matches, and read the diff",
        golden.revision,
        fleet_core::wire_contract::CONTRACT_REVISION,
    );
    let expected = golden.types;

    let mut complaints = Vec::new();
    for (ty, want) in &expected {
        match actual.get(ty) {
            None => complaints.push(format!(
                "{ty} is no longer in the contract — the desktop stopped \
                 deserialising it, or this test stopped covering it"
            )),
            Some(got) if got != want => {
                let gone: Vec<_> = want.iter().filter(|k| !got.contains(k)).collect();
                let fresh: Vec<_> = got.iter().filter(|k| !want.contains(k)).collect();
                complaints.push(format!(
                    "{ty} changed on the wire:\n  \
                     no longer sent: {gone:?}\n  \
                     newly sent:     {fresh:?}"
                ));
            }
            Some(_) => {}
        }
    }
    for ty in actual.keys() {
        if !expected.contains_key(ty) {
            complaints.push(format!("{ty} is new — add it to {GOLDEN_PATH}"));
        }
    }

    assert!(
        complaints.is_empty(),
        "the hub's wire names no longer match what the desktop expects.\n\n{}\n\n\
         A field that is no longer sent under the name above does NOT fail to \
         parse: Task 2 put #[serde(default)] on every optional field, because \
         the hub's ok_json_compact strips nulls. It silently becomes None, and \
         the desktop renders plausible wrong data with nothing in the log. If \
         the rename is deliberate, regenerate with \
         `{REGEN_ENV}=1 cargo test -p claude-fleet --lib contract` and read the \
         diff.",
        complaints.join("\n\n")
    );
}

/// `SessionRow` is the type the whole sidebar is made of, and the one whose
/// forty keys nothing else would notice losing. Its list is a literal here,
/// not only in the golden, so that a regenerate cannot quietly accept a
/// change to it.
#[test]
fn a_session_rows_wire_names_are_these_exact_forty_four() {
    let expected = [
        "account_uuid",
        "ci_status",
        "claude_session_id",
        "claude_status",
        "context_pct",
        "created_at",
        "current_activity",
        "effort_level",
        "friendly_name",
        "host_alias",
        "id",
        "idle_since",
        "kind",
        "last_activity_at",
        "last_playbook_at",
        "last_prompt",
        "last_stop_at",
        "last_turn_at",
        "lost_at",
        "notes",
        "parent_session_id",
        "pr_url",
        "project_id",
        "reviews_session_id",
        "safe_kill_detail",
        "safe_kill_nonce",
        "safe_kill_requested_at",
        "safe_kill_state",
        "started_at",
        "status",
        "stuck_kind",
        "stuck_since",
        "tags",
        "tmux_name",
        "turn_seq",
        "usage_cache_read_tokens",
        "usage_cache_write_tokens",
        "usage_cost_micros",
        "usage_input_tokens",
        "usage_model",
        "usage_output_tokens",
        "usage_updated_at",
        "worktree_id",
        "worktree_key",
    ];
    let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    assert_eq!(expected.len(), 44, "the list above lost or gained a line");
    assert_eq!(wire_keys(&sample_session()), expected);
}

/// The three where a rename is both **invisible and harmful**, because for
/// each of them an absent key means "everything is fine".
///
/// This is deliberately separate from the whole-contract test above: those
/// three names must fail their own test with their own message, so whoever
/// sees it red reads what breaks rather than a diff of forty keys.
#[test]
fn the_three_fields_where_absent_means_fine_keep_their_names() {
    let keys = wire_keys(&sample_session());
    for (field, consequence) in [
        (
            "lost_at",
            "None means \"not lost\", so a ghost session renders as live in \
             the sidebar and in every filter",
        ),
        (
            "stuck_kind",
            "None means \"not stuck\", so a session wedged on an auth menu or \
             an OOM looks healthy and no stuck playbook fires",
        ),
        (
            "context_pct",
            "None means \"no reading\", so the context-red warning never fires",
        ),
    ] {
        assert!(
            keys.contains(&field.to_string()),
            "SessionRow no longer sends `{field}` under that name. \
             Unlike most fields this does not fail to parse and is not \
             visibly wrong — {consequence}. Keys actually sent: {keys:?}"
        );
    }
}

/// The Task 2 review's NIT 10: `#[serde(default)]` on a **`Vec`** is the same
/// falsely-reassuring shape as the three `Option`s above, and the report's
/// blast-radius list missed it.
///
/// `WorktreeOccupancy::occupants` is the one that bites: empty means "no live
/// session is using this worktree", which the UI reads as *free to delete*. A
/// rename there turns an occupied worktree into a deletable one — the same
/// class of harm as a ghost rendering as live, arrived at through a different
/// serde attribute.
#[test]
fn the_empty_vec_defaults_that_read_as_good_news_keep_their_names() {
    for (ty, keys, field, consequence) in [
        (
            "WorktreeOccupancy",
            wire_keys(&sample_occupancy()),
            "occupants",
            "empty means \"no live session is using this worktree\", which the \
             UI offers as free to delete — so a rename makes an OCCUPIED \
             worktree look deletable",
        ),
        (
            "SessionRow",
            wire_keys(&sample_session()),
            "tags",
            "empty means \"untagged\", so every tag filter silently matches \
             nothing and the sidebar looks merely unlabelled",
        ),
        (
            "ProjectTreeRow",
            wire_keys(&sample_project_tree()),
            "worktrees",
            "empty means \"this project has no worktrees\", which is a normal \
             state and therefore invisible",
        ),
        (
            "ConvTurn",
            wire_keys(&sample_conversation().turns.into_iter().next().unwrap()),
            "items",
            "empty means \"the assistant said nothing this turn\", so the \
             Conversation tab renders a prompt with no reply",
        ),
    ] {
        assert!(
            keys.contains(&field.to_string()),
            "{ty} no longer sends `{field}` under that name. Like the three \
             Option fields above this does not fail to parse and is not \
             visibly wrong — {consequence}. Keys actually sent: {keys:?}"
        );
    }
}

/// The hazard itself, demonstrated rather than asserted away.
///
/// A round-trip test cannot catch this, which is the entire reason the tests
/// above pin literal strings. Here a hub renames `lost_at` to `lostAt`; the
/// desktop's `SessionRow` parses the row **successfully**, and the session it
/// hands the sidebar is one that has never been lost.
#[test]
fn a_renamed_optional_field_defaults_silently_which_is_the_whole_point() {
    let ghost = sample_session();
    assert!(ghost.lost_at.is_some(), "the fixture must start out lost");

    let mut wire = serde_json::to_value(&ghost).unwrap();
    let obj = wire.as_object_mut().unwrap();
    // What a rename on the hub side looks like from here.
    let value = obj.remove("lost_at").unwrap();
    obj.insert("lostAt".into(), value);

    let parsed: SessionRow =
        serde_json::from_value(wire).expect("a renamed optional field does NOT fail to parse");
    assert_eq!(
        parsed.lost_at, None,
        "if this ever stops being None, serde started rejecting the rename \
         and the pinned-name tests could be relaxed"
    );
    assert_eq!(
        parsed.id, ghost.id,
        "the rest of the row came through intact, which is what makes it \
         look like a healthy read"
    );
}

/// A non-optional field is still a hard error, so the contract tests only
/// have to carry the optional ones. Stated as a test so the claim is checked
/// rather than believed.
#[test]
fn a_renamed_required_field_still_fails_loudly() {
    let mut wire = serde_json::to_value(sample_session()).unwrap();
    let obj = wire.as_object_mut().unwrap();
    let value = obj.remove("tmux_name").unwrap();
    obj.insert("tmuxName".into(), value);
    let err = serde_json::from_value::<SessionRow>(wire)
        .expect_err("a missing required field must not parse");
    assert!(err.to_string().contains("tmux_name"), "{err}");
}

/// `TaskRow::nonce` and `worker_claude_session_id` are `skip_serializing`, so
/// they are absent from the wire **by design**: a client must not be able to
/// forge a task's completion marker. Pinned here so that "make TaskRow
/// round-trip properly" never turns into sending them.
#[test]
fn a_task_row_never_puts_its_nonce_on_the_wire() {
    let keys = wire_keys(&sample_task());
    for secret in ["nonce", "worker_claude_session_id"] {
        assert!(
            !keys.contains(&secret.to_string()),
            "TaskRow now sends `{secret}`. It is skip_serializing because a \
             client must not be able to forge the FLEET_TASK_DONE marker."
        );
    }
    let back: TaskRow = serde_json::from_value(serde_json::to_value(sample_task()).unwrap())
        .expect("a TaskRow read back from a hub must still parse");
    assert_eq!(back.nonce, "", "a hub-read TaskRow carries no nonce");
    assert_eq!(back.worker_claude_session_id, None);
}

// ── the wire-contract revision ──────────────────────────────────────────────

/// The golden file's own tie to `CONTRACT_REVISION`, isolated from the big
/// field-name test above so it fails with its own message rather than being
/// buried in a wall of key-diff complaints.
#[test]
fn the_goldens_revision_matches_the_wire_contract_constant() {
    let golden = golden_on_disk();
    assert_eq!(
        golden.revision,
        fleet_core::wire_contract::CONTRACT_REVISION,
        "hub_contract.golden.json says revision {}, but \
         fleet_core::wire_contract::CONTRACT_REVISION is {}",
        golden.revision,
        fleet_core::wire_contract::CONTRACT_REVISION,
    );
}

#[test]
fn hub_contract_revision_reads_the_field() {
    assert_eq!(
        hub_contract_revision(r#"{"contract":3,"version":"1.0"}"#),
        3
    );
}

#[test]
fn hub_contract_revision_defaults_to_zero_when_the_field_is_absent() {
    assert_eq!(
        hub_contract_revision(r#"{"version":"0.2.20","now":1,"kinds":["session"]}"#),
        0,
        "a hub built before this mechanism existed sends nothing"
    );
}

#[test]
fn hub_contract_revision_defaults_to_zero_for_unparsable_data() {
    assert_eq!(hub_contract_revision("not json"), 0);
}

#[test]
fn a_hub_below_the_minimum_is_too_old() {
    assert_eq!(classify_hub_contract(3, 5, 9), ContractFit::TooOld);
}

#[test]
fn a_hub_above_the_maximum_is_too_new() {
    assert_eq!(classify_hub_contract(10, 5, 9), ContractFit::TooNew);
}

#[test]
fn a_hub_at_either_edge_of_the_range_is_in_range() {
    assert_eq!(classify_hub_contract(5, 5, 9), ContractFit::InRange);
    assert_eq!(classify_hub_contract(9, 5, 9), ContractFit::InRange);
}

#[test]
fn todays_bounds_accept_a_hub_with_no_contract_field_and_this_builds_own_hub() {
    // The concrete claim MIN_HUB_CONTRACT/MAX_HUB_CONTRACT exist to make
    // true: an old hub (read as revision 0, see `hub_contract_revision` above)
    // and this build's own hub (`fleet_core::wire_contract::CONTRACT_REVISION`)
    // are both inside `[MIN_HUB_CONTRACT, MAX_HUB_CONTRACT]` today.
    assert_eq!(
        classify_hub_contract(0, MIN_HUB_CONTRACT, MAX_HUB_CONTRACT),
        ContractFit::InRange
    );
    assert_eq!(
        classify_hub_contract(
            fleet_core::wire_contract::CONTRACT_REVISION,
            MIN_HUB_CONTRACT,
            MAX_HUB_CONTRACT
        ),
        ContractFit::InRange
    );
}

#[test]
fn types_that_lost_fields_reports_only_renames_and_removals() {
    let old = BTreeMap::from([
        ("A".to_string(), vec!["x".to_string(), "y".to_string()]),
        ("B".to_string(), vec!["z".to_string()]),
    ]);
    // A gained a field (additive — not reported), B's only field was
    // renamed (reported), and C is new outright (not reported: nothing
    // about it was LOST).
    let new = BTreeMap::from([
        (
            "A".to_string(),
            vec!["x".to_string(), "y".to_string(), "w".to_string()],
        ),
        ("B".to_string(), vec!["zz".to_string()]),
        ("C".to_string(), vec!["q".to_string()]),
    ]);
    let lost = types_that_lost_fields(&old, &new);
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert!(lost[0].starts_with("B:"), "{lost:?}");
}

#[test]
fn types_that_lost_fields_reports_a_type_that_vanished_outright() {
    let old = BTreeMap::from([("A".to_string(), vec!["x".to_string()])]);
    let new = BTreeMap::new();
    let lost = types_that_lost_fields(&old, &new);
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert!(lost[0].contains("the whole type is gone"), "{lost:?}");
}

#[test]
fn types_that_lost_fields_is_empty_for_a_purely_additive_change() {
    let old = BTreeMap::from([("A".to_string(), vec!["x".to_string()])]);
    let new = BTreeMap::from([
        ("A".to_string(), vec!["x".to_string(), "y".to_string()]),
        ("B".to_string(), vec!["z".to_string()]),
    ]);
    assert!(types_that_lost_fields(&old, &new).is_empty());
}
