//! The field-name contract tests. See [`super`] for why they exist.

use super::*;
use fleet_core::service::health::Health;
use fleet_core::service::projects::ProjectTreeRow;
use fleet_core::service::repo_read::{
    Branch, ChangedFile, Commit, CommitDetail, FileContent, FileDiff, GitRef, RepoTree,
};
use fleet_core::service::transcript::{ContextView, ConvItem, ConvTurn, Conversation};
use fleet_core::service::tunnel::TunnelHealth;
use fleet_core::service::usage::DayUsage;
use fleet_core::service::worktrees::{HostWorktrees, WorktreeOccupancy, WorktreeOccupant};
use fleet_core::store::{
    AccountRow, ConversationRow, HostRow, PendingInput, PendingOption, ProjectRow, SessionContext,
    SessionEvent, SessionRow, SessionUsage, TaskRow, UsageTotals, WorkLinkRow, WorkSummary,
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
        lost_reason: Some("host_reboot".into()),
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
        row_version: 12,
        usage: sample_usage(),
        context: SessionContext {
            model: Some("claude-opus-5".into()),
            context_tokens: Some(120_000),
            context_window: Some(200_000),
            context_source: Some("transcript".into()),
            context_at: Some(1_725_001_000),
            context_stale: true,
            tmux_pane_id: Some("%17".into()),
        },
        pending_input: Some(PendingInput {
            kind: "permission".into(),
            question: Some("Do you want to proceed?".into()),
            options: vec![PendingOption {
                n: 1,
                label: "Yes".into(),
                selected: true,
            }],
        }),
        work: Some(WorkSummary {
            link_id: 5,
            item_id: Some(6),
            key: Some("ABC-123".into()),
            title: "Login".into(),
            source: "manual".into(),
        }),
        work_rejected: vec!["XYZ-9".into()],
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
        claude_session_id: Some("claude-uuid".into()),
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
        system: false,
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

fn sample_host_worktrees() -> HostWorktrees {
    HostWorktrees {
        host_alias: "trn".into(),
        project_id: 2,
        cloned: true,
        worktrees: vec![sample_worktree_row()],
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
        // A flapping tunnel is the case worth pinning on the wire: it is how a
        // remote operator learns the Control API is unreachable from a host.
        tunnels: BTreeMap::from([(
            "trn".to_string(),
            TunnelHealth {
                supervised: true,
                connected: false,
                consecutive_failures: 412,
                restarts: 412,
                last_exit_code: Some(255),
                last_error: Some("bind [127.0.0.1]:4180: Address already in use".into()),
                last_connected_unix: None,
                backoff_ms: 30_000,
            },
        )]),
        tunnels_flapping: 1,
    }
}

/// One row of `session_conversations` — the switcher in the Conversations
/// tab reads every field, and `current` decides which conversation the panel
/// treats as live.
fn sample_conversation_row() -> ConversationRow {
    ConversationRow {
        id: 1,
        session_id: 2,
        claude_session_id: "claude-uuid".into(),
        transcript_path: Some("/h/.claude/projects/p/claude-uuid.jsonl".into()),
        started_at: 1_725_000_000,
        ended_at: Some(1_725_000_900),
        start_source: "clear".into(),
        end_reason: Some("replaced".into()),
        model: Some("claude-opus-5".into()),
        first_prompt: Some("fix the bug".into()),
        turns: 3,
        compactions: 1,
        current: false,
    }
}

fn sample_conversation() -> Conversation {
    Conversation {
        turns: vec![ConvTurn {
            prompt: Some("hello".into()),
            at: Some("2026-09-18T10:00:00Z".into()),
            ended_at: Some("2026-09-18T10:00:05Z".into()),
            reminders: vec!["the harness stapled this on".into()],
            items: vec![
                ConvItem::Text {
                    text: "hi back".into(),
                },
                ConvItem::Tool {
                    summary: "Read(src/lib.rs)".into(),
                    error: true,
                    id: Some("toolu_1".into()),
                    name: "Read".into(),
                    target: Some("src/lib.rs".into()),
                    at: Some("2026-09-18T10:00:01Z".into()),
                    ended_at: Some("2026-09-18T10:00:02Z".into()),
                    done: true,
                },
                sample_subagent(),
            ],
        }],
        truncated: true,
        context: Some(ContextView {
            tokens: 120_000,
            window: 200_000,
            pct: 60.0,
            stale: false,
        }),
        events: vec![sample_event()],
    }
}

fn sample_subagent() -> ConvItem {
    ConvItem::Subagent {
        id: Some("toolu_2".into()),
        name: "Task".into(),
        agent_type: Some("Explore".into()),
        description: Some("find it".into()),
        result: Some("found".into()),
        error: false,
        at: Some("2026-09-18T10:00:03Z".into()),
        ended_at: Some("2026-09-18T10:00:04Z".into()),
        done: true,
    }
}

fn sample_work_link() -> WorkLinkRow {
    WorkLinkRow {
        id: 5,
        item_id: Some(6),
        ref_key: Some("ABC-123".into()),
        participant_id: Some(7),
        state: "confirmed".into(),
        source: "manual".into(),
        is_primary: true,
        created_at: 1,
        decided_at: Some(2),
        ended_at: Some(3),
        snap_host: Some("trn".into()),
        snap_tmux: Some("demo".into()),
        snap_name: Some("Fix login".into()),
        snap_project_id: Some(8),
        snap_worktree: Some("abc-123".into()),
        snap_branch: Some("abc-123-login".into()),
        snap_pr_url: Some("https://example.com/pr/1".into()),
        snap_claude_ids: Some("[\"c1\"]".into()),
        role: "work".into(),
        resumable: true,
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
    put("WorkLinkRow", wire_keys(&sample_work_link()));
    put("HostRow", wire_keys(&sample_host()));
    put("AccountRow", wire_keys(&sample_account()));
    put("SessionEvent", wire_keys(&sample_event()));
    put("TaskRow", wire_keys(&sample_task()));
    put("ProjectTreeRow", wire_keys(&sample_project_tree()));
    put("ProjectRow", wire_keys(&sample_project_row()));
    put("WorktreeRow", wire_keys(&sample_worktree_row()));
    put("WorktreeOccupancy", wire_keys(&sample_occupancy()));
    put("HostWorktrees", wire_keys(&sample_host_worktrees()));
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
    put("ConversationRow", wire_keys(&sample_conversation_row()));
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
            id: Some("toolu_1".into()),
            name: "Bash".into(),
            target: Some("t".into()),
            at: Some("2026-09-18T10:00:01Z".into()),
            ended_at: Some("2026-09-18T10:00:02Z".into()),
            done: true,
        }),
    );
    put("ConvItem::Subagent", wire_keys(&sample_subagent()));
    put(
        "ConvItem::Compact",
        wire_keys(&ConvItem::Compact {
            trigger: Some("auto".into()),
            pre_tokens: Some(150_000),
            summary: Some("s".into()),
        }),
    );
    put(
        "ConvItem::Command",
        wire_keys(&ConvItem::Command {
            name: "/model".into(),
            args: Some("opus".into()),
            output: Some("o".into()),
        }),
    );
    put(
        "ConvItem::Notification",
        wire_keys(&ConvItem::Notification {
            task_id: Some("a623962a33b4c9765".into()),
            tool_use_id: Some("toolu_1".into()),
            status: Some("completed".into()),
            summary: Some("Agent finished".into()),
            result: Some("r".into()),
            output_file: Some("/private/tmp/x/tasks/a6.output".into()),
            event: Some("e".into()),
            at: Some("2026-09-18T10:12:00Z".into()),
        }),
    );
    put(
        "ConvItem::Bash",
        wire_keys(&ConvItem::Bash {
            command: "git status".into(),
            stdout: Some("clean".into()),
            stderr: Some("".into()),
        }),
    );
    // Also the shape an item kind this build cannot read degrades to
    // (`fleet_core::service::transcript::unsupported_item`), which is a
    // `Harness` block — see `an_unknown_conv_item_kind_degrades_to_a_pinned_shape`.
    put(
        "ConvItem::Harness",
        wire_keys(&ConvItem::Harness {
            tag: "ci-monitor-event".into(),
            body: "PR #12 checks failed".into(),
        }),
    );
    put(
        "ConvItem::Interrupt",
        wire_keys(&ConvItem::Interrupt { during_tool: true }),
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
        // The decision is made BEFORE anything touches disk — see
        // `regen_verdict`'s doc comment for why the write used to come
        // first and what that let through.
        let lost = types_that_lost_fields(&old.types, &actual);
        match regen_verdict(
            lost,
            old.revision,
            fleet_core::wire_contract::CONTRACT_REVISION,
        ) {
            RegenVerdict::Refuse { lost } => {
                panic!(
                    "refusing to regenerate {GOLDEN_PATH}: a field was renamed or \
                     removed without bumping the wire-contract revision (still {}):\n\n\
                     {}\n\n\
                     Review crates/fleet-core/src/wire_contract.rs's CONTRACT_REVISION \
                     and this file's MIN_HUB_CONTRACT/MAX_HUB_CONTRACT, bump \
                     CONTRACT_REVISION, then regenerate again. {GOLDEN_PATH} was NOT \
                     written.",
                    old.revision,
                    lost.join("\n"),
                );
            }
            RegenVerdict::Write => {
                let golden = Golden {
                    revision: fleet_core::wire_contract::CONTRACT_REVISION,
                    types: actual.clone(),
                };
                let mut json = serde_json::to_string_pretty(&golden).unwrap();
                json.push('\n');
                std::fs::write(golden_abs(), json).expect("write the golden");
                // Deliberately fails after rewriting. Regenerating this file
                // is never the end of the job — someone has to read the diff
                // and decide whether the hub renaming that field was meant
                // to happen. A green run would let it pass unread, which is
                // the failure mode the whole module exists to close.
                panic!(
                    "{GOLDEN_PATH} was regenerated. Read `git diff -- {GOLDEN_PATH}`: \
                     a key that changed name is a field the desktop will silently \
                     default from here on. Then unset {REGEN_ENV} and run again."
                );
            }
        }
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
         parse: this struct puts #[serde(default)] on every optional field, \
         because the hub's ok_json_compact strips nulls. It silently becomes \
         None, and the desktop renders plausible wrong data with nothing in \
         the log. If \
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
fn a_session_rows_wire_names_are_these_exact_fifty_six() {
    let expected = [
        "account_uuid",
        "ci_status",
        "claude_session_id",
        "claude_status",
        "context_at",
        "context_pct",
        "context_source",
        "context_stale",
        "context_tokens",
        "context_window",
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
        "lost_reason",
        "model",
        "notes",
        "parent_session_id",
        "pending_input",
        "pr_url",
        "project_id",
        "reviews_session_id",
        "row_version",
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
        "tmux_pane_id",
        "turn_seq",
        "usage_cache_read_tokens",
        "usage_cache_write_tokens",
        "usage_cost_micros",
        "usage_input_tokens",
        "usage_model",
        "usage_output_tokens",
        "usage_updated_at",
        "work",
        "work_rejected",
        "worktree_id",
        "worktree_key",
    ];
    let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    assert_eq!(expected.len(), 56, "the list above lost or gained a line");
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

/// `#[serde(default)]` on a **`Vec`** is the same falsely-reassuring shape as
/// the three `Option`s above.
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
fn hub_contract_revision_defaults_to_zero_when_the_whole_frame_is_unparsable() {
    // The whole `ready` frame is not even JSON — a hub that HAS a contract to
    // report always JSON-encodes it correctly, so this is read the same as a
    // missing field, per the doc comment on `hub_contract_revision`.
    assert_eq!(hub_contract_revision("not json"), 0);
}

/// #148 finding 8: a `contract` key that is PRESENT but not a `u32` used to
/// fold to the same `0` as a genuinely absent key, which is in range —
/// silently trusting a hub sending nonsense as if it were an old hub sending
/// nothing. It must instead read as a hub too new to understand (out of
/// range on the high side), never as "everything is fine".
#[test]
fn hub_contract_revision_treats_a_present_but_unreadable_value_as_too_new() {
    for data in [
        r#"{"contract":"three","version":"1.0"}"#, // a string
        r#"{"contract":3.5,"version":"1.0"}"#,     // a float
        r#"{"contract":-1,"version":"1.0"}"#,      // negative
        r#"{"contract":18446744073709551615,"version":"1.0"}"#, // > u32::MAX
    ] {
        assert_eq!(hub_contract_revision(data), u32::MAX, "{data}");
    }
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
fn a_hub_with_no_contract_field_or_still_on_revision_1_is_now_too_old() {
    // `move_session` answering a tagged `MoveOutcome` and honouring
    // `dry_run` (wire_contract's revision-2 entry) raised MIN_HUB_CONTRACT
    // past 0 and past 1: a hub sending no `contract` field at all (read as
    // revision 0, see `hub_contract_revision` above) or one still on
    // revision 1 no longer round-trips this build's assumptions, so both
    // must now be refused rather than trusted with a silent default. This
    // is also the "too old" edge exercised through the LIVE bounds, not
    // just the pure classifier above — `MIN_HUB_CONTRACT` used to be `0`,
    // which nothing on a `u32` can fall below.
    assert_eq!(
        classify_hub_contract(0, MIN_HUB_CONTRACT, MAX_HUB_CONTRACT),
        ContractFit::TooOld
    );
    assert_eq!(
        classify_hub_contract(1, MIN_HUB_CONTRACT, MAX_HUB_CONTRACT),
        ContractFit::TooOld
    );
}

#[test]
fn a_hub_still_on_revision_2_is_now_too_old_too() {
    // Transfer 3c Task 4: `move_session` gained a `when` argument
    // (`now` | `idle` | `cancel`), and an older hub ignoring it would
    // perform a real move for `when: cancel` — cancelling a wait would
    // MOVE the session. That raised MIN_HUB_CONTRACT past 2, same as the
    // revision-2 bump raised it past 0 and 1 above: this is the live-bounds
    // edge for the new minimum, kept alongside (not instead of) the
    // revision-1 case, per the same "never delete a pin, only extend it"
    // rule that test follows.
    assert_eq!(
        classify_hub_contract(2, MIN_HUB_CONTRACT, MAX_HUB_CONTRACT),
        ContractFit::TooOld
    );
}

/// Round 20 (F6/F7): revision 4. Two additions an older hub cannot absorb
/// landed together — `ConvItem` gained the `bash` and `harness` kinds (a
/// revision-3 client fails the WHOLE `Conversation` on either, since it has
/// no tolerant path), and `session_activity` became a hub-routed tool a
/// revision-3 hub's router does not serve at all. Both are silent today, so
/// the minimum moves past 3 exactly as it moved past 2, keeping (not
/// replacing) the older pins above.
///
/// **This test fails until `MIN_HUB_CONTRACT`/`MAX_HUB_CONTRACT` in
/// `backend/contract.rs` are raised to 4 alongside
/// `fleet_core::wire_contract::CONTRACT_REVISION`** — as does
/// `todays_bounds_accept_this_builds_own_hub` below, which would otherwise
/// have this build refusing its OWN hub as too new.
#[test]
fn a_hub_still_on_revision_3_is_now_too_old_as_well() {
    assert_eq!(
        classify_hub_contract(3, MIN_HUB_CONTRACT, MAX_HUB_CONTRACT),
        ContractFit::TooOld
    );
}

#[test]
fn todays_bounds_accept_this_builds_own_hub() {
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

#[test]
fn regen_verdict_writes_a_purely_additive_change_at_the_same_revision() {
    assert_eq!(regen_verdict(vec![], 1, 1), RegenVerdict::Write);
}

#[test]
fn regen_verdict_refuses_a_lossy_change_at_the_same_revision() {
    let lost = vec!["SessionRow: lost [\"lost_at\"]".to_string()];
    assert_eq!(
        regen_verdict(lost.clone(), 1, 1),
        RegenVerdict::Refuse { lost }
    );
}

#[test]
fn regen_verdict_writes_a_lossy_change_once_the_revision_moved_past_the_old_one() {
    let lost = vec!["SessionRow: lost [\"lost_at\"]".to_string()];
    assert_eq!(regen_verdict(lost, 1, 2), RegenVerdict::Write);
}

#[test]
fn regen_verdict_refuses_a_lossy_change_even_if_the_revision_moved_backward() {
    // A hand-edited rollback does not retroactively cover a loss either.
    let lost = vec!["SessionRow: lost [\"lost_at\"]".to_string()];
    assert_eq!(
        regen_verdict(lost.clone(), 2, 1),
        RegenVerdict::Refuse { lost }
    );
}

/// The other direction of the same worry: a hub **newer** than this build
/// sending an item kind that did not exist when this desktop was compiled.
///
/// `ConvItem` is internally tagged, so serde's own answer is to fail the
/// entire `Conversation` — a parse error where a session's history should
/// be. `ConvTurn::items` degrades it to one
/// `fleet_core::service::transcript::unsupported_item` line instead, and
/// that line is a `Harness` block precisely because `harness` is a kind
/// every renderer already draws; the shape is pinned above as
/// `ConvItem::Harness`.
#[test]
fn an_unknown_conv_item_kind_degrades_to_a_pinned_shape() {
    let conv: Conversation = serde_json::from_value(serde_json::json!({
        "turns": [{
            "prompt": "hi",
            "items": [
                { "kind": "text", "text": "kept" },
                { "kind": "a_kind_from_the_future", "whatever": 1 },
            ],
        }],
        "truncated": false,
        "context": null,
        "events": [],
    }))
    .expect("a newer hub's item kind must not fail the whole conversation");
    assert_eq!(conv.turns[0].items.len(), 2, "the known item survives it");
    let placeholder = &conv.turns[0].items[1];
    assert!(
        matches!(placeholder, ConvItem::Harness { tag, .. } if tag.contains("a_kind_from_the_future")),
        "the placeholder names the kind it could not read: {placeholder:?}"
    );
    assert_eq!(
        wire_keys(placeholder),
        the_whole_contract()["ConvItem::Harness"],
        "the placeholder goes back out in the shape this file pins"
    );
}

/// A hub that predates the structured tool lines sends a `Tool` item with
/// only `summary` / `error`; the desktop must still parse it (and the
/// Conversation around it) rather than fail the whole read.
#[test]
fn an_older_hubs_tool_item_still_parses_with_defaults() {
    let item: ConvItem =
        serde_json::from_value(serde_json::json!({ "kind": "tool", "summary": "Read(x)" }))
            .expect("an older hub's tool item must parse");
    assert_eq!(
        item,
        ConvItem::Tool {
            summary: "Read(x)".into(),
            error: false,
            id: None,
            name: String::new(),
            target: None,
            at: None,
            ended_at: None,
            // Finished, not "no result": an older hub's line is history.
            done: true,
        }
    );
}
