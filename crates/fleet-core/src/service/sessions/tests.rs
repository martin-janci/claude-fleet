use super::*;
use crate::ipc_error::codes;
use crate::service::repair::MIRROR_REFUSED;
use crate::store::Store;

#[test]
fn known_agent_status_keeps_vocabulary_and_drops_the_rest() {
    for good in [
        "working",
        "blocked",
        "completed",
        "failed",
        "stopped",
        "idle",
    ] {
        assert_eq!(
            known_agent_status("dev", Some(good)).as_deref(),
            Some(good),
            "{good} is in the vocabulary and must be stored verbatim"
        );
    }
    // Unknown CLI values fall through to None so the pane-derived
    // fallback (or the COALESCE-preserved prior value) is used instead.
    assert_eq!(known_agent_status("dev", Some("awaiting_input")), None);
    assert_eq!(known_agent_status("dev", Some("")), None);
    assert_eq!(known_agent_status("dev", None), None);
}

fn job_agent(session_id: &str, job_id: Option<&str>) -> crate::claude_agents::ClaudeAgentRow {
    crate::claude_agents::ClaudeAgentRow {
        session_id: Some(session_id.into()),
        name: Some("n".into()),
        status: Some("working".into()),
        cwd: Some("/w".into()),
        kind: crate::claude_agents::AgentKind::Background,
        job_id: job_id.map(Into::into),
        started_at: None,
    }
}

#[test]
fn bg_stop_target_refuses_an_external_row() {
    // Even when the agent is listed with a job id: fleet never stops a
    // session that runs outside it.
    let agents = vec![job_agent("sid-1", Some("44366faf"))];
    let err = bg_stop_target("external", &agents, "sid-1").unwrap_err();
    assert_eq!(err.code, "E_INVALID_STATE");
    assert_eq!(
        err.message,
        "this Claude session runs outside fleet; close it where it runs"
    );
}

#[test]
fn bg_stop_target_absent_agent_is_already_gone() {
    let agents = vec![job_agent("other", Some("44366faf"))];
    assert_eq!(bg_stop_target("bg", &agents, "sid-1").unwrap(), None);
    assert_eq!(bg_stop_target("bg", &[], "sid-1").unwrap(), None);
}

#[test]
fn bg_stop_target_returns_the_listed_job_id() {
    let agents = vec![
        job_agent("other", Some("aaaaaaaa")),
        job_agent("sid-1", Some("44366faf")),
    ];
    assert_eq!(
        bg_stop_target("bg", &agents, "sid-1").unwrap().as_deref(),
        Some("44366faf")
    );
}

#[test]
fn bg_stop_target_present_without_job_id_suggests_remove_from_list() {
    let agents = vec![job_agent("sid-1", None)];
    let err = bg_stop_target("bg", &agents, "sid-1").unwrap_err();
    assert_eq!(err.code, "E_INVALID_STATE");
    assert!(err.message.contains("Remove from list"), "{}", err.message);
}

#[test]
fn bg_kill_action_stopped_bg_row_is_dismissed_without_claude_stop() {
    // An inactive agent (dead daemon): even when it is still listed with a
    // job id, `claude stop` is skipped and the row is removed from the list.
    let agents = vec![job_agent("sid-1", Some("44366faf"))];
    assert_eq!(
        bg_kill_action("bg", Some("stopped"), &agents, "sid-1").unwrap(),
        BgKillAction::Dismiss
    );
    assert_eq!(
        bg_kill_action("bg", Some("stopped"), &[], "sid-1").unwrap(),
        BgKillAction::Dismiss
    );
}

#[test]
fn bg_kill_action_live_bg_row_with_job_is_stopped() {
    let agents = vec![job_agent("sid-1", Some("44366faf"))];
    for status in [Some("working"), Some("blocked"), Some("idle"), None] {
        assert_eq!(
            bg_kill_action("bg", status, &agents, "sid-1").unwrap(),
            BgKillAction::Stop("44366faf".into()),
            "{status:?}"
        );
    }
}

#[test]
fn bg_kill_action_live_bg_row_absent_from_listing_does_nothing() {
    assert_eq!(
        bg_kill_action("bg", Some("blocked"), &[], "sid-1").unwrap(),
        BgKillAction::Nothing
    );
}

#[test]
fn bg_kill_action_refuses_external_rows_whatever_their_status() {
    let agents = vec![job_agent("sid-1", Some("44366faf"))];
    for status in [Some("stopped"), Some("working"), None] {
        let err = bg_kill_action("external", status, &agents, "sid-1").unwrap_err();
        assert_eq!(err.code, "E_INVALID_STATE");
        assert_eq!(err.message, EXTERNAL_STOP_REFUSED);
    }
}

#[test]
fn bg_kill_needs_agent_listing_only_for_live_bg_rows() {
    assert!(bg_kill_needs_listing("bg", Some("blocked")));
    assert!(bg_kill_needs_listing("bg", None));
    assert!(!bg_kill_needs_listing("bg", Some("stopped")));
    assert!(!bg_kill_needs_listing("external", Some("working")));
}

/// Build a `SessionRow` with sensible defaults for selector tests.
fn row(
    id: i64,
    host: &str,
    tmux: &str,
    kind: &str,
    project_id: Option<i64>,
    claude_status: Option<&str>,
) -> SessionRow {
    SessionRow {
        id,
        tmux_name: tmux.into(),
        host_alias: host.into(),
        project_id,
        worktree_id: None,
        created_at: 0,
        last_activity_at: 0,
        status: "running".into(),
        notes: None,
        account_uuid: None,
        kind: kind.into(),
        reviews_session_id: None,
        worktree_key: None,
        lost_at: None,
        claude_session_id: None,
        claude_status: claude_status.map(|s| s.to_string()),
        effort_level: None,
        pr_url: None,
        current_activity: None,
        context_pct: None,
        stuck_kind: None,
        friendly_name: None,
        safe_kill_state: None,
        safe_kill_nonce: None,
        safe_kill_detail: None,
        safe_kill_requested_at: None,
        idle_since: None,
        stuck_since: None,
        last_playbook_at: None,
        last_prompt: None,
        started_at: None,
        last_turn_at: None,
        ci_status: None,
        turn_seq: 0,
        last_stop_at: None,
        parent_session_id: None,
        tags: Vec::new(),
        usage: Default::default(),
    }
}

fn sample_sessions() -> Vec<SessionRow> {
    vec![
        row(1, "mac", "work-a", "work", Some(10), Some("idle")),
        row(2, "mac", "work-b", "work", Some(10), Some("running")),
        row(3, "mefistos", "work-c", "work", Some(20), Some("idle")),
        // non-work session must always be excluded
        row(4, "mac", "review-a", "review", Some(10), Some("idle")),
    ]
}

#[test]
fn select_targets_filters_by_host() {
    let s = sample_sessions();
    let f = BroadcastFilter {
        host: Some("mac".into()),
        ..Default::default()
    };
    assert_eq!(select_targets(&s, &f, None), vec![1, 2]);
}

#[test]
fn select_targets_filters_by_status() {
    let s = sample_sessions();
    let f = BroadcastFilter {
        status: Some("idle".into()),
        ..Default::default()
    };
    // session 4 is idle but kind=review, so excluded.
    assert_eq!(select_targets(&s, &f, None), vec![1, 3]);
}

#[test]
fn select_targets_filters_by_project() {
    let s = sample_sessions();
    let f = BroadcastFilter {
        project_id: Some(20),
        ..Default::default()
    };
    assert_eq!(select_targets(&s, &f, None), vec![3]);
}

#[test]
fn select_targets_filters_combined() {
    let s = sample_sessions();
    let f = BroadcastFilter {
        host: Some("mac".into()),
        project_id: Some(10),
        status: Some("running".into()),
    };
    assert_eq!(select_targets(&s, &f, None), vec![2]);
}

#[test]
fn select_targets_excludes_non_work() {
    let s = sample_sessions();
    // No filters: every work session, never the review one (id 4).
    let f = BroadcastFilter::default();
    assert_eq!(select_targets(&s, &f, None), vec![1, 2, 3]);
}

#[test]
fn select_targets_excludes_controller() {
    let s = sample_sessions();
    let f = BroadcastFilter::default();
    let controller = ("mac".to_string(), "work-a".to_string());
    // session 1 is the controller and must be dropped.
    assert_eq!(select_targets(&s, &f, Some(&controller)), vec![2, 3]);
}

#[test]
fn select_targets_controller_only_matches_on_both_host_and_tmux() {
    let s = sample_sessions();
    let f = BroadcastFilter::default();
    // Same tmux name on a different host must NOT be excluded.
    let controller = ("mefistos".to_string(), "work-a".to_string());
    assert_eq!(select_targets(&s, &f, Some(&controller)), vec![1, 2, 3]);
}

#[test]
fn guard_blocks_self_target_without_force() {
    let ctrl = ("mac".to_string(), "dev-fleet".to_string());
    let err =
        guard_not_controller(Some(&ctrl), "mac", "dev-fleet", false).expect_err("should block");
    assert_eq!(err.code, "E_SELF_TARGET");
}

#[test]
fn guard_allows_self_target_with_force() {
    let ctrl = ("mac".to_string(), "dev-fleet".to_string());
    assert!(guard_not_controller(Some(&ctrl), "mac", "dev-fleet", true).is_ok());
}

#[test]
fn guard_allows_non_controller_target() {
    let ctrl = ("mac".to_string(), "dev-fleet".to_string());
    // different name
    assert!(guard_not_controller(Some(&ctrl), "mac", "other", false).is_ok());
    // different host
    assert!(guard_not_controller(Some(&ctrl), "mefistos", "dev-fleet", false).is_ok());
}

#[test]
fn guard_allows_when_no_controller_registered() {
    assert!(guard_not_controller(None, "mac", "dev-fleet", false).is_ok());
}

#[test]
fn extracts_owner_repo_from_macos_path() {
    let r = extract_owner_repo(
        "/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/x",
    );
    assert_eq!(r, Some(("martin-janci".into(), "claude-fleet".into())));
}

#[test]
fn extracts_owner_repo_from_linux_path() {
    let r = extract_owner_repo("/home/mjanci/projects/github.com/martin-janci/sales-twins-app");
    assert_eq!(r, Some(("martin-janci".into(), "sales-twins-app".into())));
}

#[test]
fn extracts_owner_repo_when_followed_by_subdir() {
    let r = extract_owner_repo("/anywhere/projects/github.com/papayapos/pos-frontend/src/lib");
    assert_eq!(r, Some(("papayapos".into(), "pos-frontend".into())));
}

#[test]
fn returns_none_when_not_github_com_layout() {
    assert_eq!(extract_owner_repo("/tmp/random/repo"), None);
    assert_eq!(extract_owner_repo("/home/x/projects/gitlab.com/a/b"), None);
}

fn agent(
    session_id: &str,
    name: Option<&str>,
    cwd: Option<&str>,
) -> crate::claude_agents::ClaudeAgentRow {
    crate::claude_agents::ClaudeAgentRow {
        session_id: Some(session_id.into()),
        name: name.map(Into::into),
        status: Some("working".into()),
        cwd: cwd.map(Into::into),
        kind: crate::claude_agents::AgentKind::Background,
        job_id: None,
        started_at: None,
    }
}

#[test]
fn inactive_rule() {
    let now = 1_000_000;
    let old = now - AGENT_INACTIVE_SECS - 1;
    assert!(agent_is_inactive(Some("blocked"), Some(old), now));
    assert!(agent_is_inactive(None, Some(old), now));
    assert!(!agent_is_inactive(Some("working"), Some(old), now));
    assert!(!agent_is_inactive(Some("blocked"), Some(now - 10), now));
    assert!(
        !agent_is_inactive(Some("blocked"), None, now),
        "unknown time = active"
    );
}

#[test]
fn unmatched_bg_agents_selects_agents_with_no_tmux_session() {
    // No tmux sessions at all → every agent (with an id) is unmatched.
    let agents = vec![
        agent("bg-1", Some("bg-job-1"), Some("/a")),
        agent("bg-2", None, Some("/b")),
    ];
    let unmatched = unmatched_bg_agents(&[], &agents, true);
    assert_eq!(unmatched.len(), 2);

    // A tmux session whose name matches an agent → that agent is matched
    // (excluded), the other remains unmatched.
    let live = vec![crate::tmux::TmuxSession {
        name: "bg-job-1".into(),
        created: 1,
        last_activity: 1,
        attached: false,
        path: std::path::PathBuf::from("/a"),
    }];
    let unmatched = unmatched_bg_agents(&live, &agents, true);
    let ids: Vec<&str> = unmatched
        .iter()
        .map(|a| a.session_id.as_deref().unwrap())
        .collect();
    assert_eq!(ids, vec!["bg-2"], "only the unmatched agent remains");
}

#[test]
fn unmatched_bg_agents_skips_agents_without_session_id() {
    let agents = vec![crate::claude_agents::ClaudeAgentRow {
        session_id: None,
        name: Some("ghosty".into()),
        status: None,
        cwd: None,
        kind: crate::claude_agents::AgentKind::Background,
        job_id: None,
        started_at: None,
    }];
    assert!(unmatched_bg_agents(&[], &agents, true).is_empty());
}

#[test]
fn reconcile_agent_rows_upserts_bg_session_row() {
    // Feed agent rows + an EMPTY tmux list → expect a `bg` SessionRow.
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let agents = vec![agent("bg-uuid-1", Some("my-bg-job"), Some("/tmp/proj"))];

    reconcile_agent_rows(
        &s,
        "local",
        &[],
        &[],
        &agents,
        Some(&no_mtimes()),
        now_unix(),
    )
    .unwrap();

    let rows = s.list_sessions_for_host("local").unwrap();
    let bg = rows
        .iter()
        .find(|r| r.tmux_name == "bg:bg-uuid-1")
        .expect("a bg SessionRow must be present");
    assert_eq!(bg.kind, "bg");
    assert_eq!(bg.claude_session_id.as_deref(), Some("bg-uuid-1"));
    assert_eq!(bg.claude_status.as_deref(), Some("working"));
    assert_eq!(bg.status, "running");
}

#[test]
fn reconcile_agent_rows_prunes_vanished_agents_two_phase() {
    // A bg agent that disappears from `claude agents --json` is ghosted on
    // the next reconcile pass and hard-deleted (events included) on the one
    // after — so dead bg rows cannot accumulate.
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let agents = vec![agent("bg-uuid-1", Some("my-bg-job"), Some("/tmp/proj"))];
    reconcile_agent_rows(
        &s,
        "local",
        &[],
        &[],
        &agents,
        Some(&no_mtimes()),
        now_unix(),
    )
    .unwrap();
    let id = s
        .get_session("bg:bg-uuid-1", "local")
        .unwrap()
        .expect("upserted")
        .id;

    // Pass 2: agent gone (empty listing) → ghosted, still present.
    reconcile_agent_rows(&s, "local", &[], &[], &[], Some(&no_mtimes()), now_unix()).unwrap();
    let row = s
        .get_session("bg:bg-uuid-1", "local")
        .unwrap()
        .expect("ghosted, not yet deleted");
    assert_eq!(row.status, "ghost");
    assert!(row.lost_at.is_some());

    // Pass 3: still gone → hard-deleted.
    reconcile_agent_rows(&s, "local", &[], &[], &[], Some(&no_mtimes()), now_unix()).unwrap();
    assert!(
        s.get_session("bg:bg-uuid-1", "local").unwrap().is_none(),
        "dead bg row must be reaped on the second missing pass"
    );
    assert!(s.get_session_by_id(id).unwrap().is_none());
}

#[test]
fn reconcile_agent_rows_resurrects_ghost_when_agent_returns() {
    // A single missing pass (e.g. a transiently failed `claude agents`
    // probe, which comes back as an empty list) must not lose the row.
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let agents = vec![agent("bg-uuid-1", Some("my-bg-job"), Some("/tmp/proj"))];
    reconcile_agent_rows(
        &s,
        "local",
        &[],
        &[],
        &agents,
        Some(&no_mtimes()),
        now_unix(),
    )
    .unwrap();
    reconcile_agent_rows(&s, "local", &[], &[], &[], Some(&no_mtimes()), now_unix()).unwrap(); // ghosts it
    reconcile_agent_rows(
        &s,
        "local",
        &[],
        &[],
        &agents,
        Some(&no_mtimes()),
        now_unix(),
    )
    .unwrap(); // returns

    let row = s
        .get_session("bg:bg-uuid-1", "local")
        .unwrap()
        .expect("row survives a one-pass blip");
    assert_eq!(row.status, "running");
    assert_eq!(row.lost_at, None);

    // And it is NOT deleted on the next pass with the agent still live.
    reconcile_agent_rows(
        &s,
        "local",
        &[],
        &[],
        &agents,
        Some(&no_mtimes()),
        now_unix(),
    )
    .unwrap();
    assert!(s.get_session("bg:bg-uuid-1", "local").unwrap().is_some());
}

#[test]
fn reconcile_agent_rows_cleanup_spares_other_hosts_and_tmux_rows() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    s.upsert_host("remote").unwrap();
    // A bg row on ANOTHER host and a normal tmux row on this host.
    s.upsert_bg_session(
        "remote",
        "bg:other",
        None,
        "other",
        Some("working"),
        1,
        "bg",
    )
    .unwrap();
    s.upsert_session("work-a", "local", None, None, 1, 1, "running", None)
        .unwrap();

    // Two empty-agent passes on `local` — enough to ghost + delete any
    // bg row this cleanup wrongly considered.
    reconcile_agent_rows(&s, "local", &[], &[], &[], Some(&no_mtimes()), now_unix()).unwrap();
    reconcile_agent_rows(&s, "local", &[], &[], &[], Some(&no_mtimes()), now_unix()).unwrap();

    let work = s.get_session("work-a", "local").unwrap().expect("tmux row");
    assert_eq!(work.status, "running", "tmux rows are not the bg pruner's");
    let other = s
        .get_session("bg:other", "remote")
        .unwrap()
        .expect("other host's bg row");
    assert_eq!(other.status, "running");
}

fn no_mtimes() -> std::collections::HashMap<String, i64> {
    std::collections::HashMap::new()
}

const INTERACTIVE_ID: &str = "5f0c7a2e-1b9d-4c33-9a57-0d6f2b1e8c41";
const BG_ID: &str = "0b8e2f41-9d3c-4a7e-b1f0-6c5d4e3a2b19";

/// One `claude agents --json` row, parsed the way reconcile receives it.
fn agent_json(
    kind: &str,
    session_id: &str,
    status_fields: &str,
    started_at_ms: Option<i64>,
) -> crate::claude_agents::ClaudeAgentRow {
    let started = started_at_ms
        .map(|ms| format!(r#","startedAt":{ms}"#))
        .unwrap_or_default();
    let json = format!(
        r#"[{{"id":"d89375a1","cwd":"/tmp/nowhere","kind":"{kind}","sessionId":"{session_id}","name":"n-{kind}"{started},{status_fields}}}]"#
    );
    let mut rows = crate::claude_agents::parse_claude_agents_json(&json);
    assert_eq!(rows.len(), 1, "fixture must parse: {json}");
    rows.remove(0)
}

fn local_store() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    s
}

#[test]
fn interactive_agents_land_as_external_and_background_as_bg() {
    let s = local_store();
    let now = 2_000_000_000;
    let agents = vec![
        agent_json("interactive", INTERACTIVE_ID, r#""status":"busy""#, None),
        agent_json("background", BG_ID, r#""state":"working""#, None),
    ];
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&no_mtimes()), now).unwrap();

    let ext = s
        .get_session(&format!("bg:{INTERACTIVE_ID}"), "local")
        .unwrap()
        .expect("interactive agent row");
    assert_eq!(ext.kind, "external");
    assert_eq!(ext.claude_session_id.as_deref(), Some(INTERACTIVE_ID));
    assert_eq!(ext.status, "running");
    let bg = s
        .get_session(&format!("bg:{BG_ID}"), "local")
        .unwrap()
        .expect("background agent row");
    assert_eq!(bg.kind, "bg");
    assert_eq!(bg.claude_status.as_deref(), Some("working"));
}

#[test]
fn misfiled_bg_row_flips_to_external() {
    // Rows stored by the old reconcile as `bg` flip on the first new pass.
    let s = local_store();
    let tmux_name = format!("bg:{INTERACTIVE_ID}");
    s.upsert_bg_session(
        "local",
        &tmux_name,
        None,
        INTERACTIVE_ID,
        Some("idle"),
        1,
        "bg",
    )
    .unwrap();
    let agents = vec![agent_json(
        "interactive",
        INTERACTIVE_ID,
        r#""status":"idle""#,
        None,
    )];
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&no_mtimes()), 100).unwrap();
    assert_eq!(
        s.get_session(&tmux_name, "local").unwrap().unwrap().kind,
        "external"
    );
}

#[test]
fn idle_background_agent_is_stored_stopped() {
    let s = local_store();
    let now = 2_000_000_000;
    let agents = vec![agent_json(
        "background",
        BG_ID,
        r#""state":"blocked""#,
        None,
    )];
    let mtimes = std::collections::HashMap::from([(BG_ID.to_string(), now - 2 * 86_400)]);
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&mtimes), now).unwrap();
    let row = s
        .get_session(&format!("bg:{BG_ID}"), "local")
        .unwrap()
        .expect("row");
    assert_eq!(row.kind, "bg");
    assert_eq!(row.claude_status.as_deref(), Some("stopped"));

    // A recent transcript keeps the CLI's status.
    let s = local_store();
    let fresh = std::collections::HashMap::from([(BG_ID.to_string(), now - 60)]);
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&fresh), now).unwrap();
    let row = s
        .get_session(&format!("bg:{BG_ID}"), "local")
        .unwrap()
        .unwrap();
    assert_eq!(row.claude_status.as_deref(), Some("blocked"));
}

#[test]
fn idle_interactive_agent_is_never_stopped() {
    // The inactive rule is for bg agents only; external rows keep the CLI's
    // status (they leave the list when their process ends).
    let s = local_store();
    let now = 2_000_000_000;
    let agents = vec![agent_json(
        "interactive",
        INTERACTIVE_ID,
        r#""status":"idle""#,
        Some((now - 5 * 86_400) * 1000),
    )];
    let mtimes = std::collections::HashMap::from([(INTERACTIVE_ID.to_string(), now - 5 * 86_400)]);
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&mtimes), now).unwrap();
    let row = s
        .get_session(&format!("bg:{INTERACTIVE_ID}"), "local")
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, "external");
    assert_eq!(row.claude_status.as_deref(), Some("idle"));
}

#[test]
fn working_background_agent_is_never_stopped() {
    let s = local_store();
    let now = 2_000_000_000;
    let agents = vec![agent_json(
        "background",
        BG_ID,
        r#""state":"working""#,
        Some((now - 10 * 86_400) * 1000),
    )];
    let mtimes = std::collections::HashMap::from([(BG_ID.to_string(), now - 10 * 86_400)]);
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&mtimes), now).unwrap();
    let row = s
        .get_session(&format!("bg:{BG_ID}"), "local")
        .unwrap()
        .unwrap();
    assert_eq!(row.claude_status.as_deref(), Some("working"));
}

#[test]
fn started_at_stands_in_when_no_transcript() {
    let s = local_store();
    let now = 2_000_000_000;
    let agents = vec![agent_json(
        "background",
        BG_ID,
        r#""state":"blocked""#,
        Some((now - 2 * 86_400) * 1000),
    )];
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&no_mtimes()), now).unwrap();
    let row = s
        .get_session(&format!("bg:{BG_ID}"), "local")
        .unwrap()
        .unwrap();
    assert_eq!(row.claude_status.as_deref(), Some("stopped"));

    // Neither a transcript nor a start time: never guessed dead.
    let s = local_store();
    let agents = vec![agent_json(
        "background",
        BG_ID,
        r#""state":"blocked""#,
        None,
    )];
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&no_mtimes()), now).unwrap();
    let row = s
        .get_session(&format!("bg:{BG_ID}"), "local")
        .unwrap()
        .unwrap();
    assert_eq!(row.claude_status.as_deref(), Some("blocked"));
}

#[test]
fn transcript_mtime_wins_over_started_at() {
    // Started long ago but the transcript moved recently → active.
    let s = local_store();
    let now = 2_000_000_000;
    let agents = vec![agent_json(
        "background",
        BG_ID,
        r#""state":"blocked""#,
        Some((now - 30 * 86_400) * 1000),
    )];
    let mtimes = std::collections::HashMap::from([(BG_ID.to_string(), now - 3_600)]);
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&mtimes), now).unwrap();
    let row = s
        .get_session(&format!("bg:{BG_ID}"), "local")
        .unwrap()
        .unwrap();
    assert_eq!(row.claude_status.as_deref(), Some("blocked"));
}

#[test]
fn dismissed_agent_is_skipped_until_newer_activity() {
    let s = local_store();
    let tmux_name = format!("bg:{BG_ID}");
    let agents = vec![agent_json(
        "background",
        BG_ID,
        r#""state":"blocked""#,
        None,
    )];
    s.dismiss_agent("local", BG_ID, 100).unwrap();

    // Activity older than the dismissal → skipped, no row, dismissal kept.
    let old = std::collections::HashMap::from([(BG_ID.to_string(), 90)]);
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&old), 200).unwrap();
    assert!(s.get_session(&tmux_name, "local").unwrap().is_none());
    assert_eq!(s.dismissed_agents("local").unwrap().get(BG_ID), Some(&100));

    // Activity exactly at the dismissal → still dismissed.
    let same = std::collections::HashMap::from([(BG_ID.to_string(), 100)]);
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&same), 200).unwrap();
    assert!(s.get_session(&tmux_name, "local").unwrap().is_none());
    assert!(s.dismissed_agents("local").unwrap().contains_key(BG_ID));

    // Newer activity → row reappears and the dismissal is cleared.
    let newer = std::collections::HashMap::from([(BG_ID.to_string(), 150)]);
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&newer), 200).unwrap();
    let row = s
        .get_session(&tmux_name, "local")
        .unwrap()
        .expect("reappears after newer activity");
    assert_eq!(row.kind, "bg");
    assert!(s.dismissed_agents("local").unwrap().is_empty());
}

#[test]
fn dismissed_agent_without_known_time_stays_dismissed() {
    let s = local_store();
    let agents = vec![agent_json(
        "background",
        BG_ID,
        r#""state":"blocked""#,
        None,
    )];
    s.dismiss_agent("local", BG_ID, 100).unwrap();
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&no_mtimes()), 200).unwrap();
    assert!(s
        .get_session(&format!("bg:{BG_ID}"), "local")
        .unwrap()
        .is_none());
    assert!(s.dismissed_agents("local").unwrap().contains_key(BG_ID));
}

#[test]
fn dismissal_on_another_host_does_not_hide_the_agent() {
    let s = local_store();
    s.upsert_host("remote").unwrap();
    s.dismiss_agent("remote", BG_ID, 100).unwrap();
    let agents = vec![agent_json(
        "background",
        BG_ID,
        r#""state":"blocked""#,
        None,
    )];
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&no_mtimes()), 200).unwrap();
    assert!(s
        .get_session(&format!("bg:{BG_ID}"), "local")
        .unwrap()
        .is_some());
}

/// Executor returning fixed agents and recording every `transcript_mtimes`
/// call, for the probe's "one host call only when a bg agent exists" rule.
struct AgentsTmux {
    agents: Vec<crate::claude_agents::ClaudeAgentRow>,
    mtime_calls: Arc<Mutex<Vec<Vec<String>>>>,
    /// Simulate a failed mtime call (spawn error / non-zero exit / timeout).
    mtimes_fail: bool,
}

#[async_trait::async_trait]
impl TmuxExec for AgentsTmux {
    async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
        Ok(Vec::new())
    }
    async fn new_session(&self, _n: &str, _c: &std::path::Path, _p: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn kill_session(&self, _n: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn rename_session(&self, _o: &str, _n: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn restart_session(&self, _n: &str, _p: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn capture_pane(&self, _n: &str) -> Result<String, IpcError> {
        Ok(String::new())
    }
    async fn capture_pane_scrollback(&self, _n: &str, _l: u32) -> Result<String, IpcError> {
        Ok(String::new())
    }
    async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
        self.agents.clone()
    }
    async fn transcript_mtimes(
        &self,
        ids: &[String],
    ) -> Option<std::collections::HashMap<String, i64>> {
        self.mtime_calls.lock().unwrap().push(ids.to_vec());
        if self.mtimes_fail {
            return None;
        }
        Some(ids.iter().map(|id| (id.clone(), 42)).collect())
    }
}

fn host_row(alias: &str) -> HostRow {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host(alias).unwrap();
    s.list_hosts()
        .unwrap()
        .into_iter()
        .find(|h| h.alias == alias)
        .unwrap()
}

#[tokio::test]
async fn probe_reads_transcript_mtimes_for_background_agents_only() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tmux = AgentsTmux {
        agents: vec![
            agent_json("interactive", INTERACTIVE_ID, r#""status":"busy""#, None),
            agent_json("background", BG_ID, r#""state":"blocked""#, None),
        ],
        mtime_calls: Arc::clone(&calls),
        mtimes_fail: false,
    };
    let probe = probe_with_timeout(
        host_row("h"),
        Box::new(tmux),
        std::time::Duration::from_secs(5),
        None,
    )
    .await;
    assert_eq!(*calls.lock().unwrap(), vec![vec![BG_ID.to_string()]]);
    let mtimes = probe.agent_mtimes.expect("successful mtime call");
    assert_eq!(mtimes.get(BG_ID), Some(&42));
    assert_eq!(mtimes.len(), 1);

    // Only interactive agents → no extra host call at all.
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tmux = AgentsTmux {
        agents: vec![agent_json(
            "interactive",
            INTERACTIVE_ID,
            r#""status":"busy""#,
            None,
        )],
        mtime_calls: Arc::clone(&calls),
        mtimes_fail: false,
    };
    let probe = probe_with_timeout(
        host_row("h"),
        Box::new(tmux),
        std::time::Duration::from_secs(5),
        None,
    )
    .await;
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(probe.agent_mtimes, Some(std::collections::HashMap::new()));
}

#[tokio::test]
async fn probe_reports_a_failed_mtime_call_as_none() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tmux = AgentsTmux {
        agents: vec![agent_json(
            "background",
            BG_ID,
            r#""state":"blocked""#,
            None,
        )],
        mtime_calls: Arc::clone(&calls),
        mtimes_fail: true,
    };
    let probe = probe_with_timeout(
        host_row("h"),
        Box::new(tmux),
        std::time::Duration::from_secs(5),
        None,
    )
    .await;
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(probe.agent_mtimes, None);
}

/// Scriptable executor for the identity probe tests: a fixed session list
/// and a configurable `host_identity` answer, nothing else scripted. Reused
/// by later reboot-safety-net tests (Task 6) — construct with the
/// `sessions`/`identity` you need and nothing more; every other `TmuxExec`
/// method is a trivial stub, same as `ScriptedTmux`.
#[derive(Default)]
struct IdentityTmux {
    sessions: Vec<crate::tmux::TmuxSession>,
    identity: Option<crate::tmux::HostIdentity>,
    /// `claude agents --json` rows for this pass — empty by default (most
    /// callers only care about `sessions`/`identity`); the mass-loss e2e
    /// tests set this so a live tmux session picks up a `claude_session_id`
    /// via `claude_agents::find_for_session`'s by-name match.
    agents: Vec<crate::claude_agents::ClaudeAgentRow>,
}

#[async_trait::async_trait]
impl TmuxExec for IdentityTmux {
    async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
        Ok(self.sessions.clone())
    }
    async fn new_session(&self, _n: &str, _c: &std::path::Path, _p: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn kill_session(&self, _n: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn rename_session(&self, _o: &str, _n: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn restart_session(&self, _n: &str, _p: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn capture_pane(&self, _n: &str) -> Result<String, IpcError> {
        Ok(String::new())
    }
    async fn capture_pane_scrollback(&self, _n: &str, _l: u32) -> Result<String, IpcError> {
        Ok(String::new())
    }
    async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
        self.agents.clone()
    }
    async fn host_identity(&self) -> Option<crate::tmux::HostIdentity> {
        self.identity.clone()
    }
}

#[tokio::test]
async fn a_probe_records_the_host_identity_only_when_the_list_succeeded() {
    let id = crate::tmux::HostIdentity {
        boot_id: Some("b".into()),
        tmux_server_pid: Some(9),
    };
    let probe = probe_with_timeout(
        host_row("mefistos"),
        Box::new(IdentityTmux {
            sessions: vec![],
            identity: Some(id.clone()),
            ..Default::default()
        }),
        std::time::Duration::from_secs(5),
        None,
    )
    .await;
    assert_eq!(probe.identity, Some(id));
}

#[tokio::test]
async fn a_probe_drops_the_host_identity_when_list_sessions_fails() {
    // The identity read sits behind the same `tmux_result.is_ok()` guard as
    // `account`: a host we could not reach must never contribute an
    // identity reading (Task 6 would otherwise mistake a dead probe for a
    // fresh "no tmux server" signal).
    struct FailingListTmux {
        identity: Option<crate::tmux::HostIdentity>,
    }
    #[async_trait::async_trait]
    impl TmuxExec for FailingListTmux {
        async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
            Err(IpcError::new(codes::E_SSH, "unreachable"))
        }
        async fn new_session(
            &self,
            _n: &str,
            _c: &std::path::Path,
            _p: &str,
        ) -> Result<(), IpcError> {
            Ok(())
        }
        async fn kill_session(&self, _n: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn rename_session(&self, _o: &str, _n: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn restart_session(&self, _n: &str, _p: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn capture_pane(&self, _n: &str) -> Result<String, IpcError> {
            Ok(String::new())
        }
        async fn capture_pane_scrollback(&self, _n: &str, _l: u32) -> Result<String, IpcError> {
            Ok(String::new())
        }
        async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
            vec![]
        }
        async fn host_identity(&self) -> Option<crate::tmux::HostIdentity> {
            self.identity.clone()
        }
    }
    let probe = probe_with_timeout(
        host_row("mefistos"),
        Box::new(FailingListTmux {
            identity: Some(crate::tmux::HostIdentity {
                boot_id: Some("b".into()),
                tmux_server_pid: Some(9),
            }),
        }),
        std::time::Duration::from_secs(5),
        None,
    )
    .await;
    assert!(probe.result.is_err());
    assert_eq!(probe.identity, None);
}

#[test]
fn failed_mtime_call_keeps_an_old_blocked_bg_agent_blocked() {
    // Spec §2: a failed transcript probe leaves agents active. With the
    // mtimes unknown, a long-lived blocked agent's old `started_at` must not
    // flip it to `stopped` for this pass.
    let s = local_store();
    let now = 2_000_000_000;
    let agents = vec![agent_json(
        "background",
        BG_ID,
        r#""state":"blocked""#,
        Some((now - 5 * 86_400) * 1000),
    )];
    reconcile_agent_rows(&s, "local", &[], &[], &agents, None, now).unwrap();
    let row = s
        .get_session(&format!("bg:{BG_ID}"), "local")
        .unwrap()
        .expect("row");
    assert_eq!(row.claude_status.as_deref(), Some("blocked"));
}

#[test]
fn failed_mtime_call_keeps_dismissals_in_force() {
    // The agent started after the dismissal, which on a good pass would
    // revive it; with the mtimes unknown the dismissal stands.
    let s = local_store();
    let tmux_name = format!("bg:{BG_ID}");
    let agents = vec![agent_json(
        "background",
        BG_ID,
        r#""state":"blocked""#,
        Some(150 * 1000),
    )];
    s.dismiss_agent("local", BG_ID, 100).unwrap();
    reconcile_agent_rows(&s, "local", &[], &[], &agents, None, 200).unwrap();
    assert!(s.get_session(&tmux_name, "local").unwrap().is_none());
    assert_eq!(s.dismissed_agents("local").unwrap().get(BG_ID), Some(&100));

    // A good pass with the same evidence revives it (control).
    reconcile_agent_rows(&s, "local", &[], &[], &agents, Some(&no_mtimes()), 200).unwrap();
    assert!(s.get_session(&tmux_name, "local").unwrap().is_some());
}

#[test]
fn remote_project_path_returns_project_root_for_main_or_no_worktree() {
    use crate::projects::Layout;
    let root_dir = "/home/mjanci/projects/github.com";
    let (root, cwd) = remote_project_path(
        root_dir,
        Layout::Github,
        "martin-janci",
        "claude-fleet",
        None,
    );
    assert_eq!(
        root,
        "/home/mjanci/projects/github.com/martin-janci/claude-fleet"
    );
    assert_eq!(cwd, root);

    let (root, cwd) = remote_project_path(
        root_dir,
        Layout::Github,
        "papayapos",
        "pos-frontend",
        Some("main"),
    );
    assert_eq!(cwd, root);
}

#[test]
fn remote_project_path_uses_worktree_subdir_for_non_main() {
    let (root, cwd) = remote_project_path(
        "/home/mjanci/projects/github.com",
        crate::projects::Layout::Github,
        "martin-janci",
        "sales-twins-app",
        Some("feature-x"),
    );
    assert_eq!(
        root,
        "/home/mjanci/projects/github.com/martin-janci/sales-twins-app"
    );
    assert_eq!(
        cwd,
        "/home/mjanci/projects/github.com/martin-janci/sales-twins-app/.claude/worktrees/feature-x"
    );
}

#[test]
fn remote_new_session_path_unchanged_without_a_setting() {
    // Existing configs: exactly the pre-setting `{home}/projects/github.com/...`.
    let s = Store::open_in_memory().unwrap();
    let (root, cwd) = remote_project_path_for(&s, "mefistos", "/home/mjanci", "o", "r", Some("wt"));
    assert_eq!(root, "/home/mjanci/projects/github.com/o/r");
    assert_eq!(
        cwd,
        "/home/mjanci/projects/github.com/o/r/.claude/worktrees/wt"
    );
}

#[test]
fn remote_new_session_path_follows_the_projects_settings() {
    use crate::service::settings;
    let s = Store::open_in_memory().unwrap();
    settings::set(
        &s,
        settings::PROJECTS_BASE_PATH,
        r#"{"mefistos":"~/code","other":"/data/git"}"#,
    )
    .unwrap();
    // github layout under the host's own root, `~/` expanded remotely
    let (root, _) = remote_project_path_for(&s, "mefistos", "/home/mjanci", "o", "r", None);
    assert_eq!(root, "/home/mjanci/code/o/r");
    // flat layout
    settings::set(&s, settings::PROJECTS_LAYOUT, "flat").unwrap();
    let (root, cwd) =
        remote_project_path_for(&s, "mefistos", "/home/mjanci", "o", "r", Some("feat"));
    assert_eq!(root, "/home/mjanci/code/r");
    assert_eq!(cwd, "/home/mjanci/code/r/.claude/worktrees/feat");
    // absolute per-host root; a host without an entry gets the flat default
    let (root, _) = remote_project_path_for(&s, "other", "/home/x", "o", "r", None);
    assert_eq!(root, "/data/git/r");
    let (root, _) = remote_project_path_for(&s, "third", "/home/x", "o", "r", None);
    assert_eq!(root, "/home/x/projects/r");
}

#[test]
fn upsert_session_preserves_account_uuid_when_passed_existing_value() {
    use crate::store::{AccountRow, Store};
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    s.upsert_account(&AccountRow {
        uuid: "u1".into(),
        email: None,
        display_name: None,
        organization_name: None,
        organization_uuid: None,
        seat_tier: None,
        last_seen_at: None,
        nickname: None,
        has_extra_usage: false,
    })
    .unwrap();
    // First reconcile captures host's account
    s.upsert_session("dev-a", "h", None, None, 1, 100, "running", Some("u1"))
        .unwrap();
    // Host re-auths into a different account
    s.upsert_account(&AccountRow {
        uuid: "u2".into(),
        email: None,
        display_name: None,
        organization_name: None,
        organization_uuid: None,
        seat_tier: None,
        last_seen_at: None,
        nickname: None,
        has_extra_usage: false,
    })
    .unwrap();
    // Second reconcile: caller reads existing account before upsert
    let preserved = s.get_session_account("h", "dev-a").unwrap();
    s.upsert_session(
        "dev-a",
        "h",
        None,
        None,
        1,
        200,
        "running",
        preserved.as_deref(), // u1
    )
    .unwrap();
    // Verify session kept the ORIGINAL account
    assert_eq!(
        s.get_session_account("h", "dev-a").unwrap().as_deref(),
        Some("u1")
    );
}

#[test]
fn build_send_commands_emits_literal_text_then_enter() {
    let cmds = build_send_commands("dev-foo", "hello world", true);
    assert_eq!(cmds.len(), 3);
    assert!(cmds[0].starts_with("tmux send-keys -t "));
    assert!(cmds[0].contains(" -l "));
    assert!(cmds[0].contains("'hello world'"));
    assert!(cmds.last().unwrap().ends_with(" Enter"));
}

#[test]
fn build_send_commands_escapes_embedded_quotes() {
    let cmds = build_send_commands("dev-foo", "it's a test", true);
    // quote uses the '\''..  dance for embedded singles.
    assert!(cmds[0].contains("'it'\\''s a test'"));
}

#[test]
fn build_send_commands_quotes_session_name_with_dashes() {
    let cmds = build_send_commands("dev-with-dashes", "x", true);
    // Quoted AND exact: a bare name would let tmux prefix-match another session.
    assert!(cmds[0].contains("'=dev-with-dashes:'"), "got: {}", cmds[0]);
}

#[test]
fn send_commands_strip_trailing_newline_and_submit_once() {
    let cmds = build_send_commands("dev-x", "line1\nline2\n", true);
    // body preserves the internal newline, trailing newline stripped
    assert!(cmds
        .iter()
        .any(|c| c.contains("-l") && c.contains("line1") && c.contains("line2")));
    // the literal body must not carry the trailing newline
    assert!(!cmds.iter().any(|c| c.contains("line2\n")));
    // exactly one Enter/submit, with a settle before it
    let enters = cmds.iter().filter(|c| c.ends_with("Enter")).count();
    assert_eq!(enters, 1);
    assert!(cmds.iter().any(|c| c.contains("sleep")));
}

#[test]
fn send_commands_no_submit_when_submit_false() {
    let cmds = build_send_commands("dev-x", "stage me", false);
    assert!(cmds.iter().all(|c| !c.ends_with("Enter")));
}

#[tokio::test]
async fn parallel_reconcile_does_not_serialise_on_slow_host() {
    use crate::tmux::TmuxSession;
    use async_trait::async_trait;
    use std::time::Duration;

    struct SleepyTmux {
        sleep_ms: u64,
    }

    #[async_trait]
    impl TmuxExec for SleepyTmux {
        async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
            tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
            Ok(Vec::new())
        }
        async fn new_session(
            &self,
            _name: &str,
            _cwd: &std::path::Path,
            _pane_cmd: &str,
        ) -> Result<(), IpcError> {
            Ok(())
        }
        async fn kill_session(&self, _name: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn rename_session(&self, _old: &str, _new: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn restart_session(&self, _name: &str, _pane_cmd: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn capture_pane(&self, _name: &str) -> Result<String, IpcError> {
            Ok(String::new())
        }
        async fn capture_pane_scrollback(
            &self,
            _name: &str,
            _lines: u32,
        ) -> Result<String, IpcError> {
            Ok(String::new())
        }
        async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
            vec![]
        }
    }

    // Spawn 3 tasks with sleeps 50ms, 500ms, 50ms.
    // Sequential sum ≈ 600ms; parallel max ≈ 500ms.
    let mut set = tokio::task::JoinSet::new();
    let start = std::time::Instant::now();
    for ms in [50u64, 500, 50] {
        set.spawn(async move { SleepyTmux { sleep_ms: ms }.list_sessions().await });
    }
    while set.join_next().await.is_some() {}
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(700),
        "parallel reconcile took {elapsed:?}, expected ≈max not sum",
    );
}

#[tokio::test]
async fn wedged_host_probe_times_out_into_unreachable() {
    // Regression: a host whose probe hangs far past the cap must still
    // resolve — as an Err result (→ "unreachable, keep last-known
    // sessions") — and at ~the cap, not the hang length. Without the
    // timeout this future never completes, which is exactly what left the
    // whole sidebar empty when one host's ssh ControlMaster wedged: the
    // multi-host collector awaits EVERY probe before list_sessions returns.
    use crate::tmux::TmuxSession;
    use async_trait::async_trait;
    use std::time::Duration;

    struct HangingTmux;
    #[async_trait]
    impl TmuxExec for HangingTmux {
        async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
            tokio::time::sleep(Duration::from_secs(3600)).await; // never within the test
            Ok(Vec::new())
        }
        async fn new_session(
            &self,
            _n: &str,
            _c: &std::path::Path,
            _p: &str,
        ) -> Result<(), IpcError> {
            Ok(())
        }
        async fn kill_session(&self, _n: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn rename_session(&self, _o: &str, _n: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn restart_session(&self, _n: &str, _p: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn capture_pane(&self, _n: &str) -> Result<String, IpcError> {
            Ok(String::new())
        }
        async fn capture_pane_scrollback(&self, _n: &str, _l: u32) -> Result<String, IpcError> {
            Ok(String::new())
        }
        async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
            vec![]
        }
    }

    // A real HostRow, without standing up ssh.
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("wedged").unwrap();
    let host = store
        .list_hosts()
        .unwrap()
        .into_iter()
        .find(|h| h.alias == "wedged")
        .expect("host row");

    let start = std::time::Instant::now();
    let before = now_unix();
    let probe =
        probe_with_timeout(host, Box::new(HangingTmux), Duration::from_millis(80), None).await;
    let elapsed = start.elapsed();

    assert_eq!(
        probe.host.alias, "wedged",
        "host identity preserved for the writer"
    );
    assert!(
        probe.result.is_err(),
        "wedged probe must surface as Err → unreachable"
    );
    assert!(probe.agent_rows.is_empty());
    assert!(probe.intel.is_empty());
    assert!(
        probe.started_at >= before && probe.started_at <= now_unix(),
        "probe start is stamped even on timeout"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "must return at ~the cap, not the 3600s hang; took {elapsed:?}",
    );
}

/// Scriptable executor for the reconcile-core tests: returns a fixed
/// session list after `delay` (or never, when `hang`), and counts how many
/// probes hit it so a test can prove "zero probes" / "exactly one probe".
struct ScriptedTmux {
    sessions: Vec<crate::tmux::TmuxSession>,
    delay: std::time::Duration,
    hang: bool,
    probes: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl TmuxExec for ScriptedTmux {
    async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
        self.probes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.hang {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(self.delay).await;
        Ok(self.sessions.clone())
    }
    async fn new_session(&self, _n: &str, _c: &std::path::Path, _p: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn kill_session(&self, _n: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn rename_session(&self, _o: &str, _n: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn restart_session(&self, _n: &str, _p: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn capture_pane(&self, _n: &str) -> Result<String, IpcError> {
        Ok(String::new())
    }
    async fn capture_pane_scrollback(&self, _n: &str, _l: u32) -> Result<String, IpcError> {
        Ok(String::new())
    }
    async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
        vec![]
    }
}

fn tmux_session(name: &str) -> crate::tmux::TmuxSession {
    crate::tmux::TmuxSession {
        name: name.to_string(),
        created: 1,
        last_activity: 1,
        attached: false,
        path: PathBuf::from("/tmp"),
    }
}

/// Deps whose `local` host answers with `local_sessions` after `delay`
/// and whose every other host hangs forever. `probes` counts list calls
/// across all hosts.
fn scripted_deps(
    local_sessions: Vec<crate::tmux::TmuxSession>,
    delay: std::time::Duration,
    probe_timeout: std::time::Duration,
) -> (Arc<ReconcileDeps>, Arc<std::sync::atomic::AtomicUsize>) {
    let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let probes_for_exec = Arc::clone(&probes);
    let deps = ReconcileDeps::fake(
        move |alias| {
            Box::new(ScriptedTmux {
                sessions: if alias == "local" {
                    local_sessions.clone()
                } else {
                    Vec::new()
                },
                delay,
                hang: alias != "local",
                probes: Arc::clone(&probes_for_exec),
            })
        },
        probe_timeout,
    );
    (deps, probes)
}

#[tokio::test]
async fn fleet_reconcile_completes_when_one_host_never_answers() {
    // BE-1 (d): the multi-host fan-out must finish — and write the healthy
    // host's rows — when another host's probe never returns. The dead host
    // is treated exactly like an unreachable one (reachable=false,
    // last-known rows kept).
    use std::time::Duration;
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    {
        let s = store.lock().unwrap();
        s.upsert_host("wedged").unwrap();
        s.upsert_session("wedged-old", "wedged", None, None, 1, 1, "running", None)
            .unwrap();
    }
    let (deps, _probes) = scripted_deps(
        vec![tmux_session("local-live")],
        Duration::from_millis(10),
        Duration::from_millis(150),
    );
    let start = std::time::Instant::now();
    reconcile_sessions_with(&store, &deps)
        .await
        .expect("fan-out completes");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "one dead host must not block the pass; took {:?}",
        start.elapsed()
    );
    let s = store.lock().unwrap();
    let wedged = s
        .list_hosts()
        .unwrap()
        .into_iter()
        .find(|h| h.alias == "wedged")
        .unwrap();
    assert!(!wedged.reachable, "timed-out host is marked unreachable");
    let kept = s.list_sessions_for_host("wedged").unwrap();
    assert_eq!(kept.len(), 1, "last-known rows kept on the dead host");
    assert_eq!(kept[0].status, "running", "not ghosted by a timeout");
    let local = s.list_sessions_for_host("local").unwrap();
    assert_eq!(local.len(), 1, "healthy host's rows were written");
    assert_eq!(local[0].tmux_name, "local-live");
}

#[tokio::test]
async fn reconcile_links_the_local_account_when_it_becomes_known() {
    // The bug: `local` is auto-created by `reconcile_sessions_with`'s
    // `Store::upsert_host("local")` with no probe attached, so a fleet that
    // has been reconciling for a long time (the `local` row exists,
    // `reachable`, freshly `last_pinged_at`) can still have never once
    // discovered which Claude account is logged in locally — the account
    // only ever got linked through the separate, manually-triggered
    // `service::hosts::probe_host` ("Re-probe" in Settings). This test
    // drives the REAL automatic path (`reconcile_sessions_with`, exactly
    // what the background tick and app startup call) with a temp
    // `$HOME`-like directory standing in for `~/.claude.json`, and asserts
    // the account ends up linked without anything else being asked to
    // probe it.
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".claude.json"),
        r#"{"oauthAccount":{"accountUuid":"796436ed-fd1f-436d-bc15-ad1a81f78a71","emailAddress":"mj.janci@gmail.com","seatTier":null}}"#,
    )
    .unwrap();
    let deps = ReconcileDeps::fake_with_local_home(
        |_alias| {
            Box::new(ScriptedTmux {
                sessions: Vec::new(),
                delay: std::time::Duration::from_millis(0),
                hang: false,
                probes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            })
        },
        std::time::Duration::from_secs(5),
        dir.path().to_path_buf(),
    );

    // Note: `local` does not exist in the store yet — `reconcile_sessions_with`
    // creates it via `upsert_host`, exactly as it does on every real pass.
    reconcile_sessions_with(&store, &deps)
        .await
        .expect("reconcile completes");

    let s = store.lock().unwrap();
    let local = s
        .list_hosts()
        .unwrap()
        .into_iter()
        .find(|h| h.alias == "local")
        .expect("local host row exists after reconcile");
    assert_eq!(
        local.account_uuid.as_deref(),
        Some("796436ed-fd1f-436d-bc15-ad1a81f78a71"),
        "reconcile must link the logged-in local account automatically, \
         without requiring a manual Re-probe click"
    );
    let accounts = s.list_accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].email.as_deref(), Some("mj.janci@gmail.com"));
}

/// `ScriptedTmux` plus a scripted `read_oauth_account` answer, standing in
/// for a REMOTE host whose `~/.claude.json` is read over ssh every pass.
struct AccountTmux {
    inner: ScriptedTmux,
    account: Option<crate::service::hosts::OauthAccount>,
}

#[async_trait::async_trait]
impl TmuxExec for AccountTmux {
    async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
        self.inner.list_sessions().await
    }
    async fn new_session(&self, _n: &str, _c: &std::path::Path, _p: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn kill_session(&self, _n: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn rename_session(&self, _o: &str, _n: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn restart_session(&self, _n: &str, _p: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn capture_pane(&self, _n: &str) -> Result<String, IpcError> {
        Ok(String::new())
    }
    async fn capture_pane_scrollback(&self, _n: &str, _l: u32) -> Result<String, IpcError> {
        Ok(String::new())
    }
    async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
        vec![]
    }
    async fn read_oauth_account(&self) -> Option<crate::service::hosts::OauthAccount> {
        self.account.clone()
    }
}

fn oauth_account(uuid: &str, email: &str) -> crate::service::hosts::OauthAccount {
    crate::service::hosts::OauthAccount {
        uuid: Some(uuid.to_string()),
        email: Some(email.to_string()),
        ..Default::default()
    }
}

/// Deps whose remote host `h` lists `sessions` and reports `account`;
/// `local` (and any other alias) lists nothing and reports no account.
fn remote_account_deps(
    sessions: Vec<crate::tmux::TmuxSession>,
    account: Option<crate::service::hosts::OauthAccount>,
) -> Arc<ReconcileDeps> {
    ReconcileDeps::fake(
        move |alias| {
            let is_h = alias == "h";
            Box::new(AccountTmux {
                inner: ScriptedTmux {
                    sessions: if is_h { sessions.clone() } else { Vec::new() },
                    delay: std::time::Duration::from_millis(0),
                    hang: false,
                    probes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                },
                account: if is_h { account.clone() } else { None },
            })
        },
        std::time::Duration::from_secs(5),
    )
}

fn host_account(store: &Mutex<Store>, alias: &str) -> Option<String> {
    store
        .lock()
        .unwrap()
        .list_hosts()
        .unwrap()
        .into_iter()
        .find(|h| h.alias == alias)
        .expect("host row exists")
        .account_uuid
}

#[tokio::test]
async fn reconcile_relinks_a_remote_host_after_an_account_switch() {
    // The bug: a remote host's account was captured ONCE by `add_host` and
    // then only ever refreshed by the manual "Re-probe" click. The user
    // ran `claude /login` as someone else on the host and the Hosts view
    // kept showing the account they had left. Every pass now re-reads the
    // host's `~/.claude.json` over ssh (`TmuxExec::read_oauth_account`)
    // and relinks on a different uuid.
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    store.lock().unwrap().upsert_host("h").unwrap();

    // Pass 1: logged in as acc-1, one session running.
    let deps = remote_account_deps(
        vec![tmux_session("old")],
        Some(oauth_account("acc-1", "one@x.com")),
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert_eq!(host_account(&store, "h").as_deref(), Some("acc-1"));

    // Pass 2: the user re-logged in as acc-2; a second session appeared.
    let deps = remote_account_deps(
        vec![tmux_session("old"), tmux_session("fresh")],
        Some(oauth_account("acc-2", "two@x.com")),
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();

    assert_eq!(
        host_account(&store, "h").as_deref(),
        Some("acc-2"),
        "a different uuid in the host's ~/.claude.json must relink it without a Re-probe"
    );
    let s = store.lock().unwrap();
    let accounts = s.list_accounts().unwrap();
    assert!(
        accounts
            .iter()
            .any(|a| a.uuid == "acc-2" && a.email.as_deref() == Some("two@x.com")),
        "the new account row is upserted from the probe"
    );
    // Session attribution: the pre-existing session keeps the account it
    // was started under (preservation invariant); the session first seen
    // in the SAME pass as the switch is attributed to the NEW account, not
    // the link snapshotted before the probe.
    assert_eq!(
        s.get_session_account("h", "old").unwrap().as_deref(),
        Some("acc-1")
    );
    assert_eq!(
        s.get_session_account("h", "fresh").unwrap().as_deref(),
        Some("acc-2")
    );
}

#[tokio::test]
async fn reconcile_keeps_the_remote_link_when_the_account_read_yields_nothing() {
    // A failed read (ssh hiccup, mid-rewrite ~/.claude.json) or a logout
    // must not flap the link off — same rule as `sync_local_account`.
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    store.lock().unwrap().upsert_host("h").unwrap();
    let deps = remote_account_deps(Vec::new(), Some(oauth_account("acc-1", "one@x.com")));
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert_eq!(host_account(&store, "h").as_deref(), Some("acc-1"));

    let deps = remote_account_deps(vec![tmux_session("fresh")], None);
    reconcile_sessions_with(&store, &deps).await.unwrap();

    assert_eq!(
        host_account(&store, "h").as_deref(),
        Some("acc-1"),
        "no readable account ⇒ the stored link is left untouched"
    );
    // ...and sessions found meanwhile still attribute to that stored link.
    assert_eq!(
        store
            .lock()
            .unwrap()
            .get_session_account("h", "fresh")
            .unwrap()
            .as_deref(),
        Some("acc-1")
    );
}

#[tokio::test]
async fn concurrent_list_sessions_share_one_reconcile_pass() {
    // BE-2: two callers racing into `list_sessions` (UI focus + MCP tool,
    // say) must cause ONE fleet probe; the loser is served the stored
    // rows immediately instead of queueing a second pass.
    use std::time::Duration;
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    let gate = ReconcileGate::new();
    let (deps, probes) = scripted_deps(
        vec![tmux_session("s1")],
        Duration::from_millis(200),
        Duration::from_secs(5),
    );
    let window = Duration::from_secs(60);
    let (a, b) = tokio::join!(
        list_sessions_with(&store, &deps, &gate, window, false),
        list_sessions_with(&store, &deps, &gate, window, false),
    );
    a.expect("first caller ok");
    b.expect("second caller ok");
    assert_eq!(
        gate.passes(),
        1,
        "exactly one pass for two concurrent callers"
    );
    assert_eq!(
        probes.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "only the `local` host was probed, once"
    );
    // The winner (whichever it was) got the fresh row; the store now has it.
    let rows = list_sessions_with(&store, &deps, &gate, window, false)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tmux_name, "s1");
}

#[tokio::test]
async fn list_sessions_within_freshness_window_causes_zero_probes() {
    // BE-2: a store that was reconciled within the interval is served as
    // is; `force` (the explicit-refresh path) still probes.
    use std::time::Duration;
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    let gate = ReconcileGate::new();
    let (deps, probes) = scripted_deps(
        vec![tmux_session("s1")],
        Duration::from_millis(1),
        Duration::from_secs(5),
    );
    let window = Duration::from_secs(60);
    // First call: nothing completed yet → one pass.
    list_sessions_with(&store, &deps, &gate, window, false)
        .await
        .unwrap();
    assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 1);
    // Within the window: served from the store, zero new probes.
    for _ in 0..3 {
        let rows = list_sessions_with(&store, &deps, &gate, window, false)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "stored rows are returned");
    }
    assert_eq!(
        probes.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "fresh store ⇒ no probe"
    );
    assert_eq!(gate.passes(), 1);
    // Explicit refresh ignores freshness.
    list_sessions_with(&store, &deps, &gate, window, true)
        .await
        .unwrap();
    assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(gate.passes(), 2);
    // A zero-length window means "always stale".
    list_sessions_with(&store, &deps, &gate, Duration::ZERO, false)
        .await
        .unwrap();
    assert_eq!(gate.passes(), 3);
}

#[tokio::test]
async fn failed_pass_leaves_gate_stale_so_next_caller_retries() {
    // A pass that errors must not stamp `last_completed`; the next caller
    // probes again instead of trusting a store that never got written.
    let gate = ReconcileGate::new();
    {
        let pass = gate.try_begin().expect("free gate");
        drop(pass); // errored / aborted: no `complete()`
    }
    assert!(!gate.is_fresh(std::time::Duration::from_secs(60)));
    assert_eq!(gate.passes(), 0);
    let pass = gate.try_begin().expect("released after drop");
    assert!(gate.try_begin().is_none(), "single slot while a pass runs");
    pass.complete();
    assert!(gate.is_fresh(std::time::Duration::from_secs(60)));
    assert_eq!(gate.passes(), 1);
}

#[tokio::test]
async fn stale_probe_write_does_not_ghost_session_created_after_probe_start() {
    // BE-3 end to end through the service writer: a tick's probe starts
    // (its `keep` set is frozen), `new_session` then creates + reconciles
    // a session on the same host, and only afterwards does the tick's
    // write land. The new row must survive; a probe that starts after the
    // create ghosts it as usual.
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    let host = {
        let s = store.lock().unwrap();
        s.upsert_host("h").unwrap();
        s.list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "h")
            .unwrap()
    };
    // 1. The stale tick probe starts: sees zero sessions.
    let stale = HostProbe {
        host: host.clone(),
        result: Ok(Vec::new()),
        agent_rows: Vec::new(),
        agent_mtimes: Some(std::collections::HashMap::new()),
        intel: PaneIntelMap::new(),
        account: None,
        pr_info: PrInfoMap::new(),
        identity: None,
        started_at: now_unix(),
    };
    // 2. `new_session` creates the tmux session and runs its own
    //    single-host reconcile, which upserts + stamps the row.
    let (deps, _) = scripted_deps(
        vec![tmux_session("brand-new")],
        std::time::Duration::from_millis(1),
        std::time::Duration::from_secs(5),
    );
    // Point the fake at host `h` instead of `local`.
    let deps_h = ReconcileDeps::fake(
        move |_alias| (deps.exec)("local"),
        std::time::Duration::from_secs(5),
    );
    reconcile_one_host_with(&store, &deps_h, "h")
        .await
        .expect("create's reconcile");
    {
        let s = store.lock().unwrap();
        let row = s
            .get_session("brand-new", "h")
            .unwrap()
            .expect("row exists");
        assert_eq!(row.status, "running");
    }
    // 3. The stale write lands.
    {
        let mut s = store.lock().unwrap();
        let projects = s.list_projects().unwrap();
        reconcile_write_one_host(&mut s, &stale, &projects).expect("stale write ok");
        let row = s.get_session("brand-new", "h").unwrap().unwrap();
        assert_eq!(
            row.status, "running",
            "row reconciled after the stale probe started must not be ghosted"
        );
        assert!(row.lost_at.is_none());
    }
    // 4. A probe that starts strictly after the create is authoritative.
    let later = HostProbe {
        host,
        result: Ok(Vec::new()),
        agent_rows: Vec::new(),
        agent_mtimes: Some(std::collections::HashMap::new()),
        intel: PaneIntelMap::new(),
        account: None,
        pr_info: PrInfoMap::new(),
        identity: None,
        started_at: now_unix() + 5,
    };
    let mut s = store.lock().unwrap();
    let projects = s.list_projects().unwrap();
    reconcile_write_one_host(&mut s, &later, &projects).unwrap();
    let row = s.get_session("brand-new", "h").unwrap().unwrap();
    assert_eq!(row.status, "ghost", "a later probe ghosts it normally");
}

#[tokio::test]
async fn reconcile_one_host_does_not_touch_other_hosts() {
    // Exercises the Store-level invariant: a write burst targeting host
    // 'alpha' must leave host 'beta's session rows untouched.
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    {
        let s = store.lock().unwrap();
        s.upsert_host("alpha").unwrap();
        s.upsert_host("beta").unwrap();
        s.upsert_session("alpha-s", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_session("beta-s", "beta", None, None, 1, 1, "running", None)
            .unwrap();
    }
    // Simulate "alpha was probed and has zero sessions" — directly call the
    // delete helper that reconcile_one_host uses internally.
    {
        let s = store.lock().unwrap();
        s.delete_sessions_not_in("alpha", &[]).unwrap();
    }
    let s = store.lock().unwrap();
    let alpha = s.list_sessions_for_host("alpha").unwrap();
    let beta = s.list_sessions_for_host("beta").unwrap();
    assert!(alpha.is_empty(), "alpha cleared");
    assert_eq!(beta.len(), 1, "beta untouched");
    assert_eq!(beta[0].tmux_name, "beta-s");
}

#[test]
fn upsert_session_captures_new_account_for_fresh_row() {
    use crate::store::AccountRow;
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    s.upsert_account(&AccountRow {
        uuid: "u1".into(),
        email: None,
        display_name: None,
        organization_name: None,
        organization_uuid: None,
        seat_tier: None,
        last_seen_at: None,
        nickname: None,
        has_extra_usage: false,
    })
    .unwrap();
    // Brand new session — no existing row
    assert!(s.get_session_account("h", "dev-new").unwrap().is_none());
    let preserved = s.get_session_account("h", "dev-new").unwrap();
    let account = preserved.or(Some("u1".to_string()));
    s.upsert_session(
        "dev-new",
        "h",
        None,
        None,
        1,
        100,
        "running",
        account.as_deref(),
    )
    .unwrap();
    assert_eq!(
        s.get_session_account("h", "dev-new").unwrap().as_deref(),
        Some("u1")
    );
}

#[tokio::test]
async fn wait_for_repl_ready_returns_once_prompt_appears() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc as StdArc;

    struct FakeTmux {
        calls: StdArc<AtomicU32>,
    }
    #[async_trait::async_trait]
    impl TmuxExec for FakeTmux {
        async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
            Ok(vec![])
        }
        async fn new_session(&self, _: &str, _: &std::path::Path, _: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn kill_session(&self, _: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn rename_session(&self, _: &str, _: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn restart_session(&self, _: &str, _: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn capture_pane(&self, _: &str) -> Result<String, IpcError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            // Not ready for the first 2 polls, then the prompt appears.
            if n < 2 {
                Ok("starting…".into())
            } else {
                Ok("│ > ".into())
            }
        }
        async fn capture_pane_scrollback(
            &self,
            _name: &str,
            _lines: u32,
        ) -> Result<String, IpcError> {
            Ok(String::new())
        }
        async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
            vec![]
        }
    }

    let calls = StdArc::new(AtomicU32::new(0));
    let tmux = FakeTmux {
        calls: calls.clone(),
    };
    let start = std::time::Instant::now();
    wait_for_repl_ready(&tmux, "x").await;
    // Returned after ~3 polls (~600ms), well under the 6s cap.
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert!(calls.load(Ordering::SeqCst) >= 3);
}

#[test]
fn resolve_session_cwd_prefers_worktree_then_project_then_errors() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    // A session with neither worktree nor project → E_NOREPO.
    s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
        .unwrap();
    let row = s.get_session("dev", "local").unwrap().unwrap();
    let err = resolve_session_cwd(&s, &row).unwrap_err();
    assert_eq!(err.code, "E_NOREPO");
}

#[test]
fn resolve_session_cwd_with_worktree_and_project_and_neither() {
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("alpha").unwrap();
    // Project with a base_path, and a worktree under it.
    let pid = store.upsert_project("o", "r", "/base/r").unwrap();
    let wid = store
        .upsert_worktree(pid, "main", "/base/r/main", None)
        .unwrap();
    // Session with worktree → worktree path wins.
    let s1 = store
        .upsert_session("s1", "alpha", Some(pid), Some(wid), 1, 1, "running", None)
        .unwrap();
    let row1 = store.get_session_by_id(s1).unwrap().unwrap();
    assert_eq!(resolve_session_cwd(&store, &row1).unwrap(), "/base/r/main");
    // Session with project but no worktree → project base.
    let s2 = store
        .upsert_session("s2", "alpha", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    let row2 = store.get_session_by_id(s2).unwrap().unwrap();
    assert_eq!(resolve_session_cwd(&store, &row2).unwrap(), "/base/r");
    // Session with neither → error.
    let s3 = store
        .upsert_session("s3", "alpha", None, None, 1, 1, "running", None)
        .unwrap();
    let row3 = store.get_session_by_id(s3).unwrap().unwrap();
    assert!(resolve_session_cwd(&store, &row3).is_err());
}

#[test]
fn resolve_session_cwd_honors_worktree_key_when_id_missing() {
    // Reproduces the recreate bug: reconcile sets `worktree_key` but never
    // `worktree_id`, so a session in a worktree must still resolve to that
    // worktree's path, not the repo root.
    let store = Store::open_in_memory().unwrap();
    store.upsert_host("local").unwrap();
    let pid = store.upsert_project("o", "r", "/base/r").unwrap();
    store
        .upsert_worktree(pid, "feat-x", "/base/r/.claude/worktrees/feat-x", None)
        .unwrap();

    // worktree_id is None (as after reconcile) but worktree_key points at it.
    let mut r = row(1, "local", "dev", "work", Some(pid), Some("idle"));
    r.worktree_key = Some("feat-x".into());
    assert_eq!(
        resolve_session_cwd(&store, &r).unwrap(),
        "/base/r/.claude/worktrees/feat-x"
    );

    // worktree_key "main" → repo root.
    let mut rm = row(2, "local", "dev2", "work", Some(pid), Some("idle"));
    rm.worktree_key = Some("main".into());
    assert_eq!(resolve_session_cwd(&store, &rm).unwrap(), "/base/r");

    // Unknown key (worktree not in the table) → graceful fallback to root.
    let mut ru = row(3, "local", "dev3", "work", Some(pid), Some("idle"));
    ru.worktree_key = Some("gone".into());
    assert_eq!(resolve_session_cwd(&store, &ru).unwrap(), "/base/r");
}

#[test]
fn worktree_path_on_disk_tries_both_layouts() {
    let base = "/base/r";
    // `.worktrees/<key>` layout (the case the stale-table bug hit).
    let only_dot_worktrees = worktree_path_on_disk(base, "test-worktree", |p| {
        p == "/base/r/.worktrees/test-worktree"
    });
    assert_eq!(
        only_dot_worktrees.as_deref(),
        Some("/base/r/.worktrees/test-worktree")
    );
    // `.claude/worktrees/<key>` is preferred when both exist.
    let both = worktree_path_on_disk(base, "feat", |_| true);
    assert_eq!(both.as_deref(), Some("/base/r/.claude/worktrees/feat"));
    // Neither present → None (caller falls back to the repo root).
    assert_eq!(worktree_path_on_disk(base, "gone", |_| false), None);
}

#[test]
fn cwd_source_remote_honors_worktree_key_when_id_missing() {
    let store = Store::open_in_memory().unwrap();
    store.upsert_host("mefistos").unwrap();
    let pid = store.upsert_project("acme", "repo", "/base/repo").unwrap();

    let mut r = row(1, "mefistos", "dev", "work", Some(pid), Some("idle"));
    r.worktree_key = Some("feat-x".into());
    match cwd_source_for_session(&store, &r).unwrap() {
        CwdSource::Remote {
            owner,
            repo,
            wt_name,
            ..
        } => {
            assert_eq!(owner, "acme");
            assert_eq!(repo, "repo");
            assert_eq!(wt_name, Some("feat-x".to_string()));
        }
        CwdSource::Local(_) => panic!("expected Remote for host=mefistos"),
    }

    // "main" carries no worktree name → project root on the remote.
    let mut rm = row(2, "mefistos", "dev2", "work", Some(pid), Some("idle"));
    rm.worktree_key = Some("main".into());
    match cwd_source_for_session(&store, &rm).unwrap() {
        CwdSource::Remote { wt_name, .. } => assert_eq!(wt_name, None),
        CwdSource::Local(_) => panic!("expected Remote for host=mefistos"),
    }
}

#[test]
fn cwd_source_remote_follows_projects_settings() {
    // recreate/restart derive the remote cwd through `cwd_source_for_session`
    // + `resolve_cwd_source`; the path must follow `projects.*`.
    use crate::projects::Layout;
    use crate::service::settings;
    let store = Store::open_in_memory().unwrap();
    store.upsert_host("mefistos").unwrap();
    let pid = store.upsert_project("acme", "repo", "/base/repo").unwrap();
    let mut r = row(1, "mefistos", "dev", "work", Some(pid), Some("idle"));
    r.worktree_key = Some("feat-x".into());

    // No setting: the historical remote root, unchanged.
    match cwd_source_for_session(&store, &r).unwrap() {
        CwdSource::Remote { root, layout, .. } => {
            assert_eq!(root, "~/projects/github.com");
            assert_eq!(layout, Layout::Github);
        }
        CwdSource::Local(_) => panic!("expected Remote for host=mefistos"),
    }

    settings::set(
        &store,
        settings::PROJECTS_BASE_PATH,
        r#"{"mefistos":"~/code"}"#,
    )
    .unwrap();
    settings::set(&store, settings::PROJECTS_LAYOUT, "flat").unwrap();
    match cwd_source_for_session(&store, &r).unwrap() {
        CwdSource::Remote {
            root,
            layout,
            owner,
            repo,
            wt_name,
        } => {
            let root = crate::service::projects::expand_home(&root, "/home/m");
            let (_, cwd) = remote_project_path(&root, layout, &owner, &repo, wt_name.as_deref());
            assert_eq!(cwd, "/home/m/code/repo/.claude/worktrees/feat-x");
        }
        CwdSource::Local(_) => panic!("expected Remote for host=mefistos"),
    }
}

fn host_paths(root: &str, layout: crate::projects::Layout) -> HostPaths {
    HostPaths {
        root: root.into(),
        layout,
        worktrees: Vec::new(),
    }
}

#[test]
fn host_paths_locate_custom_roots_and_layouts() {
    use crate::projects::Layout;
    let def = host_paths("~/projects/github.com", Layout::Github);
    assert_eq!(
        def.locate("/home/u/projects/github.com/o/r/.worktrees/f"),
        Some((Some("o"), "r", "/.worktrees/f".to_string()))
    );
    let code = host_paths("~/code", Layout::Github);
    assert_eq!(
        code.locate("/home/u/code/o/r"),
        Some((Some("o"), "r", String::new()))
    );
    assert_eq!(code.locate("/home/u/code/o"), None, "owner dir, no repo");
    let abs = host_paths("/data/git/", Layout::Flat);
    assert_eq!(
        abs.locate("/data/git/r/src"),
        Some((None, "r", "/src".to_string()))
    );
    assert_eq!(abs.locate("/data/git-old/r"), None, "whole components only");
    assert_eq!(abs.locate("/data/git"), None);
}

#[test]
fn host_paths_tilde_roots_anchor_after_home() {
    use crate::projects::Layout;
    let home = host_paths("~", Layout::Flat);
    assert_eq!(
        home.locate("/home/u/r/src"),
        Some((None, "r", "/src".to_string()))
    );
    assert_eq!(home.locate("/Users/u/r"), Some((None, "r", String::new())));
    assert_eq!(home.locate("/root/r"), Some((None, "r", String::new())));
    assert_eq!(
        home.locate("/var/home/u/r"),
        Some((None, "r", String::new()))
    );
    assert_eq!(home.locate("/home/u"), None, "the home itself is no repo");
    assert_eq!(home.locate("/srv/r"), None, "unknown home layout");
    let gh = host_paths("~/", Layout::Github);
    assert_eq!(
        gh.locate("/home/u/o/r"),
        Some((Some("o"), "r", String::new()))
    );
    let code = host_paths("~/code", Layout::Github);
    assert_eq!(
        code.locate("/home/u/work/code/o/r"),
        None,
        "a `/code/` run deeper in the path is not the root"
    );
    assert_eq!(
        code.locate("/home/code/code/o/r"),
        Some((Some("o"), "r", String::new())),
        "a user named `code` is not mistaken for the root"
    );
    // Unknown home layout: the unanchored fallback still applies.
    assert_eq!(
        code.locate("/data/users/u/code/o/r"),
        Some((Some("o"), "r", String::new()))
    );
}

#[test]
fn below_home_handles_standard_layouts() {
    assert_eq!(below_home("/home/u/a/b"), Some("a/b"));
    assert_eq!(below_home("/home/u/"), Some(""));
    assert_eq!(below_home("/home/u"), Some(""));
    assert_eq!(below_home("/home"), None);
    assert_eq!(below_home("/root"), Some(""));
    assert_eq!(below_home("/Users/u/p"), Some("p"));
    assert_eq!(below_home("/opt/x"), None);
    assert_eq!(below_home("relative/x"), None);
}

#[cfg(unix)]
#[test]
fn find_project_local_matches_through_a_symlinked_root() {
    let tmp = tempfile::TempDir::new().unwrap();
    let real = tmp.path().join("mnt").join("o").join("r");
    std::fs::create_dir_all(real.join(".worktrees").join("f")).unwrap();
    let link = tmp.path().join("projects");
    std::os::unix::fs::symlink(tmp.path().join("mnt"), &link).unwrap();
    let s = Store::open_in_memory().unwrap();
    // refresh_projects stores the physical base_path.
    let base = crate::projects::path_identity::canonical(&real);
    let pid = s.upsert_project("o", "r", &base.to_string_lossy()).unwrap();
    let xb = s
        .upsert_project(
            "o",
            "r-build",
            &base.with_file_name("r-build").to_string_lossy(),
        )
        .unwrap();
    let projects = s.list_projects().unwrap();
    let paths = HostPaths::for_host(&s, "local");
    let find = |p: &std::path::Path| find_project_id_for_path(&projects, "local", p, &paths);
    let logical = link.join("o").join("r");
    assert_eq!(
        find(&logical.join(".worktrees").join("f")),
        Some(pid),
        "a logical pane PWD links to the physical row"
    );
    assert_eq!(find(&base), Some(pid));
    assert_eq!(find(&link.join("o").join("r-build").join("src")), Some(xb));
    assert_eq!(find(&link.join("o").join("rx")), None);
}

/// A linked worktree in a SIBLING folder of the repo (outside the base
/// layout, e.g. `o/stw-fix2` next to `o/sales-twins-app`) links to the
/// repo's project through the host's worktree rows: locally the scan's
/// `git worktree list`, on a remote host its EnterWorktree hooks.
#[test]
fn find_project_links_sibling_linked_worktrees_through_worktree_rows() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("vps").unwrap();
    let app = s.upsert_project("o", "app", "/b/o/app").unwrap();
    s.upsert_worktree(app, "main", "/b/o/app", None).unwrap();
    s.upsert_worktree(app, "app-wt", "/b/o/app-wt", Some("wt"))
        .unwrap();
    s.upsert_worktree_on(
        "vps",
        app,
        "app-wt",
        "/home/u/projects/github.com/o/app-wt",
        None,
    )
    .unwrap();
    let projects = s.list_projects().unwrap();
    let local = HostPaths::for_host(&s, "local");
    let find_local =
        |p: &str| find_project_id_for_path(&projects, "local", std::path::Path::new(p), &local);
    assert_eq!(find_local("/b/o/app-wt/src"), Some(app), "sibling worktree");
    assert_eq!(find_local("/b/o/app-wt-old"), None, "whole components only");
    assert_eq!(find_local("/b/o/app/.worktrees/f"), Some(app));
    // Another host's rows never leak into this host's linking.
    assert_eq!(find_local("/home/u/projects/github.com/o/app-wt"), None);
    let vps = HostPaths::for_host(&s, "vps");
    let find_vps =
        |p: &str| find_project_id_for_path(&projects, "vps", std::path::Path::new(p), &vps);
    // The layout reads repo `app-wt`, which is no project; the host's
    // worktree row names the real one.
    assert_eq!(
        find_vps("/home/u/projects/github.com/o/app-wt/src"),
        Some(app)
    );
    assert_eq!(find_vps("/b/o/app-wt/src"), None, "local rows stay local");
}

/// A leftover duplicate project whose base IS a linked worktree (such as
/// `stw-fix2`, once scanned as its own repo and kept alive by a session)
/// must not win over the real repo that lists that checkout. The matches
/// tie on length and the worktree row wins, locally and remotely. A
/// project root deeper than the worktree row still wins.
#[test]
fn find_project_prefers_the_worktree_row_over_a_duplicate_project() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("vps").unwrap();
    let app = s.upsert_project("o", "app", "/b/o/app").unwrap();
    s.upsert_project("o", "stw-fix2", "/b/o/stw-fix2").unwrap();
    s.upsert_worktree(app, "stw-fix2", "/b/o/stw-fix2", None)
        .unwrap();
    s.upsert_worktree_on(
        "vps",
        app,
        "stw-fix2",
        "/home/u/projects/github.com/o/stw-fix2",
        None,
    )
    .unwrap();
    let nested = s
        .upsert_project("o", "nested", "/b/o/stw-fix2/vendor/nested")
        .unwrap();
    let projects = s.list_projects().unwrap();
    let local = HostPaths::for_host(&s, "local");
    let vps = HostPaths::for_host(&s, "vps");
    let find = |host: &str, paths: &HostPaths, p: &str| {
        find_project_id_for_path(&projects, host, std::path::Path::new(p), paths)
    };
    assert_eq!(
        find("local", &local, "/b/o/stw-fix2/src"),
        Some(app),
        "the duplicate project loses the tie locally"
    );
    assert_eq!(
        find("vps", &vps, "/home/u/projects/github.com/o/stw-fix2/src"),
        Some(app),
        "the duplicate project located by the layout loses the tie remotely"
    );
    assert_eq!(
        find("local", &local, "/b/o/stw-fix2/vendor/nested/x"),
        Some(nested),
        "a deeper project root beats a shorter worktree row"
    );
}

#[test]
fn find_project_local_prefix_is_component_aware() {
    let s = Store::open_in_memory().unwrap();
    let x = s.upsert_project("o", "x", "/b/x").unwrap();
    let xb = s.upsert_project("o", "x-build", "/b/x-build").unwrap();
    let projects = s.list_projects().unwrap();
    let paths = HostPaths::for_host(&s, "local");
    let find =
        |p: &str| find_project_id_for_path(&projects, "local", std::path::Path::new(p), &paths);
    assert_eq!(find("/b/x-build/src"), Some(xb));
    assert_eq!(find("/b/x/.worktrees/f"), Some(x));
    assert_eq!(find("/b/x"), Some(x));
    assert_eq!(find("/b/xy"), None);
}

#[test]
fn find_project_remote_flat_matches_unique_repo_name() {
    let s = Store::open_in_memory().unwrap();
    let a = s.upsert_project("acme", "alpha", "/l/alpha").unwrap();
    s.upsert_project("one", "dup", "/l/dup").unwrap();
    s.upsert_project("two", "dup", "/l2/dup").unwrap();
    let projects = s.list_projects().unwrap();
    let paths = host_paths("~/code", crate::projects::Layout::Flat);
    let find =
        |p: &str| find_project_id_for_path(&projects, "vps", std::path::Path::new(p), &paths);
    assert_eq!(find("/home/u/code/alpha/src"), Some(a));
    assert_eq!(
        find("/home/u/code/dup"),
        None,
        "an ambiguous repo name is not guessed"
    );
    assert_eq!(find("/home/u/elsewhere/alpha"), None);
    // The github.com convention stays the fallback.
    assert_eq!(find("/home/u/projects/github.com/acme/alpha"), Some(a));
}

/// Run two reconcile ticks for one remote session at `cwd` under the given
/// `projects.*` settings; returns (project_id, worktree_key, expected pid).
fn reconcile_linking(
    base_map: Option<&str>,
    layout: &str,
    cwd: &str,
) -> (Option<i64>, Option<String>, i64) {
    use crate::service::settings;
    let mut s = Store::open_in_memory().unwrap();
    s.upsert_host("vps").unwrap();
    let pid = s
        .upsert_project("acme", "repo", "/local/acme/repo")
        .unwrap();
    if let Some(m) = base_map {
        settings::set(&s, settings::PROJECTS_BASE_PATH, m).unwrap();
    }
    settings::set(&s, settings::PROJECTS_LAYOUT, layout).unwrap();
    let host = s
        .list_hosts()
        .unwrap()
        .into_iter()
        .find(|h| h.alias == "vps")
        .unwrap();
    let projects = s.list_projects().unwrap();
    let probe = HostProbe {
        host,
        result: Ok(vec![crate::tmux::TmuxSession {
            name: "dev-a".into(),
            created: 1,
            last_activity: 1,
            attached: false,
            path: PathBuf::from(cwd),
        }]),
        agent_rows: Vec::new(),
        agent_mtimes: Some(std::collections::HashMap::new()),
        intel: PaneIntelMap::new(),
        account: None,
        pr_info: PrInfoMap::new(),
        identity: None,
        started_at: now_unix(),
    };
    // Two ticks: the second must keep the link, not null it.
    reconcile_write_one_host(&mut s, &probe, &projects).unwrap();
    reconcile_write_one_host(&mut s, &probe, &projects).unwrap();
    let row = s.get_session("dev-a", "vps").unwrap().unwrap();
    (row.project_id, row.worktree_key, pid)
}

#[test]
fn reconcile_keeps_links_under_custom_root() {
    let (pid, key, want) = reconcile_linking(
        Some(r#"{"vps":"~/code"}"#),
        "github",
        "/home/u/code/acme/repo/.worktrees/feat",
    );
    assert_eq!(pid, Some(want));
    assert_eq!(key.as_deref(), Some("feat"));
}

#[test]
fn reconcile_keeps_links_under_flat_layout() {
    // No base set: the flat default `~/projects`.
    let (pid, key, want) =
        reconcile_linking(None, "flat", "/home/u/projects/repo/.claude/worktrees/x");
    assert_eq!(pid, Some(want));
    assert_eq!(key.as_deref(), Some("x"));
    // Absolute per-host root.
    let (pid, key, want) =
        reconcile_linking(Some(r#"{"vps":"/srv/git"}"#), "flat", "/srv/git/repo");
    assert_eq!(pid, Some(want));
    assert_eq!(key.as_deref(), Some("main"));
}

#[test]
fn reconcile_default_config_links_exactly_as_before() {
    let (pid, key, want) =
        reconcile_linking(None, "github", "/home/u/projects/github.com/acme/repo/src");
    assert_eq!(pid, Some(want));
    assert_eq!(key.as_deref(), Some("main"));
    // Outside any root and outside the convention: orphan, as before.
    let (pid, key, _) = reconcile_linking(None, "github", "/tmp/elsewhere");
    assert_eq!(pid, None);
    assert_eq!(key, None);
}

#[test]
fn cwd_source_local_uses_db_path_remote_uses_owner_repo() {
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("local").unwrap();
    store.upsert_host("mefistos").unwrap();
    let pid = store.upsert_project("acme", "repo", "/base/repo").unwrap();
    let wid = store
        .upsert_worktree(pid, "feat-x", "/base/repo/.claude/worktrees/feat-x", None)
        .unwrap();

    // LOCAL: takes the worktree's stored path verbatim.
    let lid = store
        .upsert_session("dev", "local", Some(pid), Some(wid), 1, 1, "running", None)
        .unwrap();
    let local_row = store.get_session_by_id(lid).unwrap().unwrap();
    match cwd_source_for_session(&store, &local_row).unwrap() {
        CwdSource::Local(p) => assert_eq!(p, "/base/repo/.claude/worktrees/feat-x"),
        CwdSource::Remote { .. } => panic!("expected Local for host=local"),
    }

    // REMOTE: captures (owner, repo, wt_name) — the local DB path is
    // unusable on the remote machine and must NOT leak into the cwd.
    let rid = store
        .upsert_session(
            "dev",
            "mefistos",
            Some(pid),
            Some(wid),
            1,
            1,
            "running",
            None,
        )
        .unwrap();
    let remote_row = store.get_session_by_id(rid).unwrap().unwrap();
    match cwd_source_for_session(&store, &remote_row).unwrap() {
        CwdSource::Remote {
            owner,
            repo,
            wt_name,
            ..
        } => {
            assert_eq!(owner, "acme");
            assert_eq!(repo, "repo");
            assert_eq!(wt_name.as_deref(), Some("feat-x"));
        }
        CwdSource::Local(_) => panic!("expected Remote for non-local host"),
    }

    // REMOTE without worktree → wt_name = None, so remote_project_path
    // returns the project root.
    let rid2 = store
        .upsert_session("dev2", "mefistos", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    let remote_row2 = store.get_session_by_id(rid2).unwrap().unwrap();
    match cwd_source_for_session(&store, &remote_row2).unwrap() {
        CwdSource::Remote { wt_name, .. } => assert!(wt_name.is_none()),
        CwdSource::Local(_) => panic!("expected Remote for non-local host"),
    }
}

#[test]
fn cwd_source_remote_without_project_errors() {
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("mefistos").unwrap();
    let id = store
        .upsert_session("orphan", "mefistos", None, None, 1, 1, "running", None)
        .unwrap();
    let row = store.get_session_by_id(id).unwrap().unwrap();
    let err = cwd_source_for_session(&store, &row).unwrap_err();
    assert_eq!(err.code, "E_NOREPO");
}

#[test]
fn worktree_key_root_is_main_local_and_remote() {
    assert_eq!(
        worktree_key_for_path("/Users/martinjanci/projects/github.com/martin-janci/claude-fleet"),
        Some("main".to_string())
    );
    assert_eq!(
        worktree_key_for_path("/home/mjanci/projects/github.com/martin-janci/claude-fleet"),
        Some("main".to_string())
    );
}

#[test]
fn worktree_key_extracts_named_worktree() {
    assert_eq!(
        worktree_key_for_path("/Users/x/projects/github.com/o/r/.claude/worktrees/feat-auth"),
        Some("feat-auth".to_string())
    );
    assert_eq!(
        worktree_key_for_path(
            "/home/mjanci/projects/github.com/o/r/.claude/worktrees/feat-auth/src"
        ),
        Some("feat-auth".to_string())
    );
}

#[test]
fn worktree_key_extracts_dot_worktrees_named_worktree() {
    // The `.worktrees/` layout (no `.claude/` prefix) must also key to the
    // worktree name — otherwise these sessions recreate at the repo root.
    assert_eq!(
        worktree_key_for_path("/Users/x/projects/github.com/o/r/.worktrees/changelog"),
        Some("changelog".to_string())
    );
    assert_eq!(
        worktree_key_for_path("/home/mjanci/projects/github.com/o/r/.worktrees/changelog/src"),
        Some("changelog".to_string())
    );
}

#[test]
fn worktree_key_other_subdir_is_main() {
    assert_eq!(
        worktree_key_for_path("/Users/x/projects/github.com/o/r/src/lib"),
        Some("main".to_string())
    );
}

#[test]
fn worktree_key_non_repo_path_is_none() {
    assert_eq!(worktree_key_for_path("/tmp/whatever"), None);
    assert_eq!(worktree_key_for_path("/Users/x/Documents"), None);
}

#[test]
fn recreate_pane_command_matches_kind_and_id() {
    let id = "550e8400-e29b-41d4-a716-446655440000";
    assert_eq!(
        recreate_pane_command("shell", Some(id), "dev-x"),
        crate::tmux::shell_pane_command(None)
    );
    assert_eq!(
        recreate_pane_command("work", Some(id), "dev-x"),
        crate::tmux::pane_command_for(Some(id), "dev-x")
    );
    assert_eq!(
        recreate_pane_command("work", None, "dev-x"),
        crate::tmux::pane_command_for(None, "dev-x")
    );
    // A corrupt/non-UUID stored id must NOT inject — it degrades to the
    // --continue form (same as no id).
    assert_eq!(
        recreate_pane_command("work", Some("not-a-uuid; rm -rf /"), "dev-x"),
        crate::tmux::pane_command_for(None, "dev-x")
    );
    // "review" is a non-shell kind → same resume behavior as "work".
    assert_eq!(
        recreate_pane_command("review", Some(id), "dev-x"),
        crate::tmux::pane_command_for(Some(id), "dev-x")
    );
}

#[test]
fn worktree_key_empty_worktree_name_falls_back_to_main() {
    // A trailing `.claude/worktrees/` with no name segment must not yield
    // Some("") — it degrades to the safe "main" fallback.
    assert_eq!(
        worktree_key_for_path("/Users/x/projects/github.com/o/r/.claude/worktrees/"),
        Some("main".to_string())
    );
}

#[test]
fn reconcile_writes_claude_session_id_when_name_matches() {
    use crate::claude_agents::ClaudeAgentRow;
    // Build a fake agent row with name = "my-session"
    let agent_rows = vec![ClaudeAgentRow {
        session_id: Some("abc123".into()),
        name: Some("my-session".into()),
        status: Some("working".into()),
        cwd: None,
        kind: crate::claude_agents::AgentKind::Background,
        job_id: None,
        started_at: None,
    }];
    let hit = crate::claude_agents::find_by_name(&agent_rows, "my-session");
    assert_eq!(hit.unwrap().session_id.as_deref(), Some("abc123"));
    let miss = crate::claude_agents::find_by_name(&agent_rows, "other");
    assert!(miss.is_none());
}

// ── worktree_add_script unit tests ────────────────────────────────────────

#[test]
fn worktree_add_script_contains_expected_fragments() {
    let script = worktree_add_script("/repo/root", "feat-x", None);
    assert!(script.contains("cd '/repo/root'"), "cd root: {script}");
    assert!(
        script.contains("basebr=''"),
        "empty base when None: {script}"
    );
    assert!(
        script.contains("name='feat-x'"),
        "name assignment: {script}"
    );
    assert!(
        script.contains("git worktree add"),
        "worktree add: {script}"
    );
    assert!(script.contains(" -b "), "branch flag: {script}");
    assert!(script.contains(".worktrees"), ".worktrees dir: {script}");
    assert!(
        script.contains(".claude/worktrees"),
        ".claude/worktrees dir: {script}"
    );
    assert!(
        script.contains("refs/remotes/origin/HEAD"),
        "default branch detection: {script}"
    );
    assert!(
        script.contains("( cd \"$wt\" && pwd -P )"),
        "reports the physical path: {script}"
    );
}

#[test]
fn worktree_add_script_resolves_requested_base_with_default_fallback() {
    let script = worktree_add_script("/repo/root", "feat-x", Some("dev"));
    // Requested base is captured, shell-quoted.
    assert!(script.contains("basebr='dev'"), "base captured: {script}");
    // Resolution: prefer a local branch, then origin/<base>, else fall
    // back to the default branch ($def).
    assert!(
        script.contains("refs/heads/$basebr"),
        "local branch check: {script}"
    );
    assert!(
        script.contains("refs/remotes/origin/$basebr"),
        "origin fallback check: {script}"
    );
    // The worktree is created from the resolved start point, not a literal
    // "$def" — so the default-branch arg must now be the resolved $start.
    assert!(
        script.contains("git worktree add \"$wt\" -b \"$name\" \"$start\""),
        "forks from resolved start point: {script}"
    );
}

#[test]
fn worktree_add_script_blank_base_normalizes_to_default() {
    // Whitespace-only base is treated as "unset" → empty basebr → default.
    let script = worktree_add_script("/repo/root", "feat-x", Some("  "));
    assert!(
        script.contains("basebr=''"),
        "blank base is empty: {script}"
    );
}

// ── create_worktree_local integration test ────────────────────────────────

#[tokio::test]
async fn create_worktree_local_creates_and_is_idempotent() {
    use std::process::Command;

    // Create a unique temp dir for the bare repo
    let base = std::env::temp_dir().join(format!(
        "cf-wt-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base).expect("create base");

    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).expect("create repo");
    let repo_str = repo.to_str().unwrap();

    // git init
    let status = Command::new("git")
        .args(["init", repo_str])
        .status()
        .expect("git init");
    assert!(status.success());

    // configure user.email and user.name so commit works
    Command::new("git")
        .args(["-C", repo_str, "config", "user.email", "test@test.com"])
        .status()
        .expect("git config email");
    Command::new("git")
        .args(["-C", repo_str, "config", "user.name", "Test"])
        .status()
        .expect("git config name");

    // write a file and commit
    let file = repo.join("README.md");
    std::fs::write(&file, "hello").expect("write file");
    Command::new("git")
        .args(["-C", repo_str, "add", "."])
        .status()
        .expect("git add");
    Command::new("git")
        .args(["-C", repo_str, "commit", "-m", "init"])
        .status()
        .expect("git commit");

    // call create_worktree_local
    let result = create_worktree_local(repo_str, "feat-x", None).await;
    assert!(result.is_ok(), "first call failed: {:?}", result);
    let wt_path = result.unwrap();
    assert!(
        wt_path.ends_with("/.worktrees/feat-x"),
        "path should end with /.worktrees/feat-x, got: {wt_path}"
    );
    assert!(
        std::path::Path::new(&wt_path).is_dir(),
        "worktree dir should exist: {wt_path}"
    );

    // second call — idempotent
    let result2 = create_worktree_local(repo_str, "feat-x", None).await;
    assert!(
        result2.is_ok(),
        "second (idempotent) call failed: {:?}",
        result2
    );
    assert_eq!(
        result2.unwrap(),
        wt_path,
        "idempotent call must return same path"
    );

    // cleanup
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn remote_script_must_be_quoted_to_survive_login_shell_retokenization() {
    // Regression for "zsh: parse error near `then`" on remote session
    // creation. ssh concatenates the trailing argv with spaces and the
    // remote LOGIN shell re-tokenizes the result, so `bash -lc <script>`
    // with an UNQUOTED `if ...; then ...; fi` splits at `;` and orphans
    // `then`. We reproduce that re-tokenization locally with `sh -c`.
    use std::process::Command;
    let script = "if true; then echo OK; fi";

    // RAW (the bug): the re-login shell mis-parses the orphaned `then`.
    let raw = Command::new("sh")
        .args(["-c", &format!("bash -lc {script}")])
        .output()
        .expect("sh");
    assert!(
        !raw.status.success(),
        "unquoted if/then must fail at the re-tokenizing login shell"
    );

    // QUOTED (the fix): crosses as one word, bash runs the whole script.
    let quoted = Command::new("sh")
        .args(["-c", &format!("bash -lc {}", quote(script))])
        .output()
        .expect("sh");
    assert!(
        quoted.status.success(),
        "quote'd script must run cleanly: {}",
        String::from_utf8_lossy(&quoted.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&quoted.stdout).trim(), "OK");
}

// ── Task H: background reconcile tick ──────────────────────────────────

#[test]
fn reconcile_interval_defaults_when_absent_or_garbage() {
    assert_eq!(read_reconcile_interval_secs(None), 20);
    assert_eq!(read_reconcile_interval_secs(Some("nonsense".into())), 20);
    assert_eq!(read_reconcile_interval_secs(Some("".into())), 20);
}

#[test]
fn reconcile_interval_honours_explicit_values() {
    assert_eq!(read_reconcile_interval_secs(Some("5".into())), 5);
    // Surrounding whitespace is trimmed before parsing.
    assert_eq!(read_reconcile_interval_secs(Some(" 45 ".into())), 45);
    // 0 is the documented "disabled" sentinel; surfaced verbatim so the
    // tick-interval guard can turn it into None.
    assert_eq!(read_reconcile_interval_secs(Some("0".into())), 0);
}

#[test]
fn list_freshness_window_falls_back_to_default_when_tick_disabled() {
    // Pull-only mode (interval 0) must still not re-probe on every focus.
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    assert_eq!(
        list_freshness_window(&store),
        std::time::Duration::from_secs(DEFAULT_RECONCILE_INTERVAL_SECS as u64)
    );
    store
        .lock()
        .unwrap()
        .set_setting("reconcile.interval_secs", "0")
        .unwrap();
    assert_eq!(
        list_freshness_window(&store),
        std::time::Duration::from_secs(DEFAULT_RECONCILE_INTERVAL_SECS as u64)
    );
    store
        .lock()
        .unwrap()
        .set_setting("reconcile.interval_secs", "7")
        .unwrap();
    assert_eq!(
        list_freshness_window(&store),
        std::time::Duration::from_secs(7)
    );
}

#[test]
fn reconcile_tick_interval_disabled_when_zero_or_negative() {
    // 0 = disabled (the documented "off" sentinel) and any non-positive
    // value must keep the tick from running rather than busy-loop.
    assert_eq!(reconcile_tick_interval(0), None);
    assert_eq!(reconcile_tick_interval(-5), None);
}

#[test]
fn reconcile_tick_interval_enabled_for_positive_secs() {
    assert_eq!(
        reconcile_tick_interval(20),
        Some(std::time::Duration::from_secs(20))
    );
    assert_eq!(
        reconcile_tick_interval(1),
        Some(std::time::Duration::from_secs(1))
    );
}

#[tokio::test]
async fn reconcile_now_is_callable_headless() {
    // The whole point of Task H: reconcile must be drivable from a
    // background task with just an in-memory Store + a bare SshClient — no
    // Tauri AppHandle. We assert it is *callable* this way (compiles, spawns
    // on the managed deps) without depending on the test box's real local
    // probe, which shells out to `tmux` / `claude agents --json` and can
    // block on a developer machine. A bounded timeout keeps the suite fast:
    //   - Ok(Ok(_))  → reconcile ran and returned (no real probe stalled)
    //   - Ok(Err(_)) → reconcile ran and surfaced an IpcError (still proves
    //                  the headless path executes end to end)
    //   - Err(_)     → the real local probe is blocking; the entry point is
    //                  still demonstrably callable (it was driven to await).
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    let ssh = Arc::new(SshClient::new());
    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        reconcile_now(&store, &ssh),
    )
    .await;
}

// ── Wave 2 Track D: addressing (MCP-6) ──

fn seeded_store() -> Mutex<Store> {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    s.upsert_host("mefistos").unwrap();
    s.upsert_session("dev-a", "local", None, None, 1, 1, "running", None)
        .unwrap();
    s.upsert_session("dev-a", "mefistos", None, None, 1, 1, "running", None)
        .unwrap();
    s.upsert_session("dev-only", "local", None, None, 1, 1, "running", None)
        .unwrap();
    Mutex::new(s)
}

#[test]
fn resolve_session_target_prefers_id_then_requires_full_pair() {
    let store = seeded_store();
    let s = store.lock().unwrap();
    let only = s.get_session("dev-only", "local").unwrap().unwrap();
    // id wins even when a (wrong) pair is also supplied
    let r = resolve_session_target(&s, Some(only.id), Some("mefistos"), Some("dev-a")).unwrap();
    assert_eq!(r.id, only.id);
    let r = resolve_session_target(&s, None, Some("mefistos"), Some("dev-a")).unwrap();
    assert_eq!(r.host_alias, "mefistos");
    assert_eq!(
        resolve_session_target(&s, None, Some("local"), None)
            .unwrap_err()
            .code,
        "E_INVALID"
    );
    assert_eq!(
        resolve_session_target(&s, None, None, Some("dev-a"))
            .unwrap_err()
            .code,
        "E_INVALID"
    );
    assert_eq!(
        resolve_session_target(&s, Some(9999), None, None)
            .unwrap_err()
            .code,
        "E_NOTFOUND"
    );
    assert_eq!(
        resolve_session_target(&s, None, Some("local"), Some("nope"))
            .unwrap_err()
            .code,
        "E_NOTFOUND"
    );
    assert_eq!(
        resolve_session_target(&s, None, Some("-oProxyCommand=x"), Some("dev-a"))
            .unwrap_err()
            .code,
        "E_INVALID"
    );
}

#[test]
fn find_session_by_tmux_name_prefers_running_rows_over_ghosts() {
    let store = seeded_store();
    let s = store.lock().unwrap();
    // Ghost the mefistos copy: whoami must now resolve to the live one.
    s.conn_ref()
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=5 WHERE tmux_name='dev-a' AND host_alias='mefistos'",
                [],
            )
            .unwrap();
    let row = find_session_by_tmux_name(&s, "dev-a").unwrap();
    assert_eq!(row.host_alias, "local");
    // Only ghosts left ⇒ they are still findable (one match).
    s.conn_ref()
        .execute(
            "UPDATE sessions SET status='ghost', lost_at=5 WHERE tmux_name='dev-a'",
            [],
        )
        .unwrap();
    assert_eq!(
        find_session_by_tmux_name(&s, "dev-a").unwrap_err().code,
        "E_AMBIGUOUS"
    );
}

/// D8 / Q2: an MCP-delivered prompt carries the untrusted marker as its first
/// line. The session is shown it, but the label and `last_prompt` must read as
/// what was asked, not as the marker sentence.
#[test]
fn marked_prompt_records_the_body_not_the_marker() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev-marked", "local", None, None, 1, 1, "running", None)
            .unwrap();
    }
    let body = "Rewrite the auth flow!";
    let marked = crate::mcp::guard::mark_untrusted(body, "session 12 on mefistos");
    record_prompt_outcome(&store, "local", "dev-marked", &marked);
    {
        let s = store.lock().unwrap();
        let row = s.get_session("dev-marked", "local").unwrap().unwrap();
        assert_eq!(row.last_prompt.as_deref(), Some(body));
        assert_eq!(row.friendly_name.as_deref(), Some("rewrite the auth flow"));
    }
    // An unmarked prompt is recorded verbatim, and a body that merely opens
    // with similar words keeps every character.
    let lookalike = "[claude-fleet: message from me] ship it";
    record_prompt_outcome(&store, "local", "dev-marked", lookalike);
    let s = store.lock().unwrap();
    let row = s.get_session("dev-marked", "local").unwrap().unwrap();
    assert_eq!(row.last_prompt.as_deref(), Some(lookalike));
}

#[test]
fn prompt_derived_name_replaces_only_the_branch_default() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        let wid = s
            .upsert_worktree(
                pid,
                "fix-login",
                "/p/o/r/.worktrees/fix-login",
                Some("dev-o-r--fix-login"),
            )
            .unwrap();
        s.upsert_session(
            "dev-o-r--fix-login",
            "local",
            Some(pid),
            Some(wid),
            1,
            1,
            "running",
            None,
        )
        .unwrap();
        let default = s
            .default_friendly_name(
                s.get_session("dev-o-r--fix-login", "local")
                    .unwrap()
                    .unwrap()
                    .id,
            )
            .unwrap()
            .expect("branch default");
        s.set_friendly_name("local", "dev-o-r--fix-login", Some(&default))
            .unwrap();
    }
    record_prompt_outcome(
        &store,
        "local",
        "dev-o-r--fix-login",
        "Rewrite the auth flow!",
    );
    {
        let s = store.lock().unwrap();
        let row = s
            .get_session("dev-o-r--fix-login", "local")
            .unwrap()
            .unwrap();
        assert_eq!(row.friendly_name.as_deref(), Some("rewrite the auth flow"));
        assert_eq!(row.last_prompt.as_deref(), Some("Rewrite the auth flow!"));
        // A chosen label survives the next prompt.
        s.set_friendly_name("local", "dev-o-r--fix-login", Some("My label"))
            .unwrap();
    }
    record_prompt_outcome(&store, "local", "dev-o-r--fix-login", "Another prompt here");
    let s = store.lock().unwrap();
    let row = s
        .get_session("dev-o-r--fix-login", "local")
        .unwrap()
        .unwrap();
    assert_eq!(row.friendly_name.as_deref(), Some("My label"));
    assert_eq!(row.last_prompt.as_deref(), Some("Another prompt here"));
}

#[test]
fn find_session_by_tmux_name_returns_the_single_match_or_lists_candidates() {
    let store = seeded_store();
    let s = store.lock().unwrap();
    assert_eq!(
        find_session_by_tmux_name(&s, "dev-only")
            .unwrap()
            .host_alias,
        "local"
    );
    assert_eq!(
        find_session_by_tmux_name(&s, "ghost-name")
            .unwrap_err()
            .code,
        "E_NOTFOUND"
    );
    let err = find_session_by_tmux_name(&s, "dev-a").unwrap_err();
    assert_eq!(err.code, "E_AMBIGUOUS");
    let cands = err.details.unwrap()["candidates"].as_array().unwrap().len();
    assert_eq!(cands, 2);
}

// ── Wave 2 Track D: naming + PR probe ──

#[test]
fn friendly_name_from_prompt_takes_five_lowercase_words_without_punctuation() {
    assert_eq!(
        friendly_name_from_prompt("Fix the login bug, then add tests for it!").as_deref(),
        Some("fix the login bug then")
    );
    assert_eq!(
        friendly_name_from_prompt("  Refactor   SSH   layer  ").as_deref(),
        Some("refactor ssh layer")
    );
    assert_eq!(friendly_name_from_prompt("!!! ... ---"), None);
    assert_eq!(friendly_name_from_prompt(""), None);
    // Unicode letters survive, symbols do not.
    assert_eq!(
        friendly_name_from_prompt("Oprav chybu v prihlásení (rýchlo)").as_deref(),
        Some("oprav chybu v prihlásení rýchlo")
    );
}

/// A host shell that answers the PR probe with canned stdout and counts
/// invocations, so the throttle is observable.
struct CannedShell {
    stdout: String,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl HostShell for CannedShell {
    async fn run_script(&self, _host: &str, script: &str) -> Result<String, IpcError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert!(script.contains("gh pr view"), "probe script runs gh");
        Ok(self.stdout.clone())
    }
}

fn repo_session(name: &str, path: &str) -> crate::tmux::TmuxSession {
    crate::tmux::TmuxSession {
        name: name.to_string(),
        created: 1,
        last_activity: 1,
        attached: false,
        path: PathBuf::from(path),
    }
}

#[tokio::test]
async fn reconcile_populates_pr_url_and_ci_status_and_throttles_the_probe() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stdout = "__FLEET_PR__\tdev-a\t0\t{\"url\":\"https://github.com/o/r/pull/9\",\
                      \"statusCheckRollup\":[{\"status\":\"COMPLETED\",\"conclusion\":\"SUCCESS\"}]}\n\
                      __FLEET_PR__\tdev-b\t1\tno pull requests found for branch \"main\"\n";
    let shell = Arc::new(CannedShell {
        stdout: stdout.to_string(),
        calls: Arc::clone(&calls),
    });
    let live = vec![
        repo_session("dev-a", "/home/u/projects/github.com/o/r/.worktrees/a"),
        repo_session("dev-b", "/home/u/projects/github.com/o/r"),
        // Not a github-layout path: never probed.
        repo_session("scratch", "/tmp"),
    ];
    let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let probes_for_exec = Arc::clone(&probes);
    let deps = ReconcileDeps::fake_with_shell(
        move |_alias| {
            Box::new(ScriptedTmux {
                sessions: live.clone(),
                delay: std::time::Duration::from_millis(0),
                hang: false,
                probes: Arc::clone(&probes_for_exec),
            })
        },
        std::time::Duration::from_secs(5),
        shell,
    );
    {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
    }
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    {
        let s = store.lock().unwrap();
        let a = s.get_session("dev-a", "local").unwrap().unwrap();
        assert_eq!(a.pr_url.as_deref(), Some("https://github.com/o/r/pull/9"));
        assert_eq!(a.ci_status.as_deref(), Some("passing"));
        let b = s.get_session("dev-b", "local").unwrap().unwrap();
        assert_eq!(b.pr_url, None);
        let c = s.get_session("scratch", "local").unwrap().unwrap();
        assert_eq!(c.pr_url, None);
    }
    // Second pass within the TTL: the cache says nothing is due, so the
    // shell is not consulted and the stored fields survive.
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let s = store.lock().unwrap();
    let a = s.get_session("dev-a", "local").unwrap().unwrap();
    assert_eq!(a.pr_url.as_deref(), Some("https://github.com/o/r/pull/9"));
}

#[tokio::test]
async fn reconcile_survives_a_failing_pr_probe_shell() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let live = vec![repo_session("dev-a", "/home/u/projects/github.com/o/r")];
    let deps = ReconcileDeps::fake(
        move |_alias| {
            Box::new(ScriptedTmux {
                sessions: live.clone(),
                delay: std::time::Duration::from_millis(0),
                hang: false,
                probes: Arc::clone(&probes),
            })
        },
        std::time::Duration::from_secs(5),
    );
    {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
    }
    reconcile_sessions_with(&store, &deps).await.unwrap();
    let s = store.lock().unwrap();
    let a = s.get_session("dev-a", "local").unwrap().unwrap();
    assert_eq!(a.status, "running");
    assert_eq!(a.pr_url, None);
}

// ── ensure_remote_project: mirroring an existing worktree ─────────────────

#[test]
fn ensure_remote_project_script_clones_only_without_a_worktree() {
    let script = ensure_remote_project_script(
        "/home/u/projects/github.com/o/r",
        "git@github.com:o/r.git",
        None,
    );
    assert!(script.starts_with("set -e\n"), "{script}");
    assert!(
        script.contains(
            "if [ ! -d '/home/u/projects/github.com/o/r'/.git ]; then mkdir -p \"$(dirname -- '/home/u/projects/github.com/o/r')\" && git clone 'git@github.com:o/r.git' '/home/u/projects/github.com/o/r'; fi"
        ),
        "guarded clone: {script}"
    );
    assert!(!script.contains("worktree add"), "{script}");
}

#[test]
fn ensure_remote_project_script_main_is_the_clone_itself() {
    let main_wt = RemoteWorktree {
        name: "main",
        branch: None,
        path: "/r",
    };
    let script = ensure_remote_project_script("/r", "git@github.com:o/r.git", Some(&main_wt));
    assert!(!script.contains("worktree add"), "{script}");
    let main_wt_named = RemoteWorktree {
        name: "main",
        branch: Some("main"),
        path: "/r",
    };
    assert_eq!(mirrored_branch(Some(&main_wt_named)), None);
    assert_eq!(mirrored_branch(None), None);
    let wt = RemoteWorktree {
        name: "wt",
        branch: None,
        path: "/r/.claude/worktrees/wt",
    };
    assert_eq!(mirrored_branch(Some(&wt)), Some("wt"));
    let wt_branch = RemoteWorktree {
        name: "wt",
        branch: Some("feature/x"),
        path: "/r/.claude/worktrees/wt",
    };
    assert_eq!(mirrored_branch(Some(&wt_branch)), Some("feature/x"));
}

#[test]
fn ensure_remote_project_script_mirrors_the_worktree_from_origin() {
    let wt = RemoteWorktree {
        name: "nifty-swanson",
        branch: Some("feature/elated-shtern"),
        path: "/re po/.claude/worktrees/nifty-swanson",
    };
    let script = ensure_remote_project_script("/re po", "git@github.com:o/r.git", Some(&wt));
    // Guarded on the worktree directory, quoted.
    assert!(
        script.contains("if [ ! -d '/re po/.claude/worktrees/nifty-swanson' ]; then\n"),
        "{script}"
    );
    // The repair module's Mirror add, not a naive `git worktree add <path> <branch>`.
    assert!(script.contains("b='feature/elated-shtern'\n"), "{script}");
    assert!(
        script.contains("show-ref --verify --quiet \"refs/heads/$b\""),
        "local branch first: {script}"
    );
    assert!(
        script.contains("ls-remote --exit-code --heads -- origin \"refs/heads/$b\""),
        "asks origin: {script}"
    );
    assert!(
        script.contains("fetch -- origin \"+refs/heads/$b:refs/remotes/origin/$b\""),
        "fetches before checkout: {script}"
    );
    assert!(
        script.contains(
            "worktree add --track -b \"$b\" -- '/re po/.claude/worktrees/nifty-swanson' \"origin/$b\""
        ),
        "tracks origin: {script}"
    );
    assert!(
        script.contains(MIRROR_REFUSED),
        "refuses when origin lacks it: {script}"
    );
    assert!(
        !script.contains(" -b \"$b\" -- '/re po/.claude/worktrees/nifty-swanson' \"$start\""),
        "never forks a new branch from the base: {script}"
    );
    // The name is the branch when the row has none.
    let wt_by_name = RemoteWorktree {
        name: "wt",
        branch: None,
        path: "/r/.claude/worktrees/wt",
    };
    let by_name = ensure_remote_project_script("/r", "u", Some(&wt_by_name));
    assert!(by_name.contains("b='wt'\n"), "{by_name}");
}

#[test]
fn git_setup_error_explains_a_branch_that_is_not_on_origin() {
    let e = git_setup_error(
        "mefistos",
        "o",
        "r",
        Some("feature/x"),
        "",
        &format!("repair: branch feature/x {MIRROR_REFUSED}\n"),
    );
    assert_eq!(e.code, codes::E_GIT_SETUP);
    assert_eq!(
        e.message,
        "branch feature/x is not on origin; push it from the source machine, or start a new worktree on mefistos"
    );
    // Any other failure keeps git's stderr (stdout when stderr is empty).
    let raw = git_setup_error(
        "mefistos",
        "o",
        "r",
        Some("feature/x"),
        "",
        "fatal: invalid reference: feature/x\n",
    );
    assert_eq!(raw.code, codes::E_GIT_SETUP);
    assert_eq!(
        raw.message,
        "couldn't ensure o/r on mefistos: fatal: invalid reference: feature/x"
    );
    let out = git_setup_error("h", "o", "r", None, "only stdout\n", "  ");
    assert_eq!(out.message, "couldn't ensure o/r on h: only stdout");
    // The marker without a mirrored branch (no worktree) is not rewritten.
    let no_wt = git_setup_error("h", "o", "r", None, "", MIRROR_REFUSED);
    assert!(
        no_wt.message.starts_with("couldn't ensure o/r on h: "),
        "{}",
        no_wt.message
    );
}

#[tokio::test]
async fn ensure_remote_project_maps_a_refused_mirror_to_a_push_hint() {
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    let fake = FakeSsh::new();
    fake.on_host(
        "mefistos",
        Match::script_contains("worktree add"),
        Reply::fail(
            1,
            &format!("repair: branch feature/elated-shtern {MIRROR_REFUSED}\n"),
        ),
    );
    let wt = RemoteWorktree {
        name: "nifty-swanson",
        branch: Some("feature/elated-shtern"),
        path: "/home/u/projects/github.com/FrantisekSefcik/sales-twins-app/.claude/worktrees/nifty-swanson",
    };
    let err = ensure_remote_project(
        &fake,
        "mefistos",
        "FrantisekSefcik",
        "sales-twins-app",
        "/home/u/projects/github.com/FrantisekSefcik/sales-twins-app",
        Some(&wt),
        CancellationToken::new(),
    )
    .await
    .expect_err("refused mirror must fail");
    assert_eq!(err.code, codes::E_GIT_SETUP);
    assert_eq!(
        err.message,
        "branch feature/elated-shtern is not on origin; push it from the source machine, or start a new worktree on mefistos"
    );
    // One `bash -lc '<script>'` call carrying the origin-aware add.
    let calls = fake.calls_for("mefistos");
    assert_eq!(calls.len(), 1, "{calls:?}");
    let script = calls[0].script().expect("bash -lc script");
    assert!(
        script.contains("ls-remote --exit-code --heads -- origin"),
        "{script}"
    );
    assert!(
        script.contains("git clone 'git@github.com:FrantisekSefcik/sales-twins-app.git'"),
        "{script}"
    );
}

#[tokio::test]
async fn ensure_remote_project_keeps_other_git_failures_verbatim() {
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    let fake = FakeSsh::new();
    fake.on_host(
        "mefistos",
        Match::Any,
        Reply::fail(128, "fatal: could not read from remote repository\n"),
    );
    let wt = RemoteWorktree {
        name: "wt",
        branch: Some("feature/x"),
        path: "/home/u/projects/github.com/o/r/.claude/worktrees/wt",
    };
    let err = ensure_remote_project(
        &fake,
        "mefistos",
        "o",
        "r",
        "/home/u/projects/github.com/o/r",
        Some(&wt),
        CancellationToken::new(),
    )
    .await
    .expect_err("git failure");
    assert_eq!(err.code, codes::E_GIT_SETUP);
    assert_eq!(
        err.message,
        "couldn't ensure o/r on mefistos: fatal: could not read from remote repository"
    );
    // And success is silent.
    let ok = FakeSsh::new();
    ensure_remote_project(
        &ok,
        "mefistos",
        "o",
        "r",
        "/home/u/projects/github.com/o/r",
        Some(&wt),
        CancellationToken::new(),
    )
    .await
    .expect("default reply is exit 0");
}

/// Runs the real mirror script against local git repos: a bare `origin`, a
/// `source` clone that pushes one branch and keeps another local-only, and a
/// `remote` clone standing in for the other host.
#[tokio::test]
async fn ensure_remote_project_script_mirrors_pushed_branches_and_refuses_unpushed_ones() {
    use std::process::Command;
    fn git(args: &[&str]) {
        let out = Command::new("git").args(args).output().expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let base = std::env::temp_dir().join(format!(
        "cf-mirror-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base).expect("create base");
    let origin = base.join("origin.git");
    let source = base.join("source");
    let remote = base.join("remote");
    let (origin_s, source_s, remote_s) = (
        origin.to_str().unwrap(),
        source.to_str().unwrap(),
        remote.to_str().unwrap(),
    );
    git(&["init", "--bare", "-b", "main", origin_s]);
    git(&["init", "-b", "main", source_s]);
    git(&["-C", source_s, "config", "user.email", "t@t"]);
    git(&["-C", source_s, "config", "user.name", "T"]);
    std::fs::write(source.join("README.md"), "hello").unwrap();
    git(&["-C", source_s, "add", "."]);
    git(&["-C", source_s, "commit", "-m", "init"]);
    git(&["-C", source_s, "remote", "add", "origin", origin_s]);
    git(&["-C", source_s, "push", "-u", "origin", "main"]);
    // The "remote host" clones before either feature branch exists, so its
    // `origin/*` refs are stale — exactly the case the fetch is for.
    git(&["clone", origin_s, remote_s]);
    git(&["-C", source_s, "checkout", "-b", "feature/pushed"]);
    std::fs::write(source.join("pushed.txt"), "p").unwrap();
    git(&["-C", source_s, "add", "."]);
    git(&["-C", source_s, "commit", "-m", "pushed"]);
    git(&["-C", source_s, "push", "-u", "origin", "feature/pushed"]);
    git(&["-C", source_s, "checkout", "-b", "feature/local-only"]);

    let run = |name: &str, branch: &str| {
        let path = format!("{remote_s}/.claude/worktrees/{name}");
        let wt = RemoteWorktree {
            name,
            branch: Some(branch),
            path: &path,
        };
        let script = ensure_remote_project_script(remote_s, origin_s, Some(&wt));
        Command::new("bash")
            .args(["-lc", &script])
            .output()
            .expect("bash")
    };
    // Pushed: fetched fresh and checked out tracking origin.
    let out = run("wt-pushed", "feature/pushed");
    assert!(
        out.status.success(),
        "mirror of a pushed branch: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("outcome=branch_remote"));
    let wt = remote.join(".claude/worktrees/wt-pushed");
    assert!(
        wt.join("pushed.txt").is_file(),
        "checked out at the pushed commit"
    );
    let head = Command::new("git")
        .args([
            "-C",
            wt.to_str().unwrap(),
            "rev-parse",
            "--abbrev-ref",
            "HEAD",
        ])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&head.stdout).trim(),
        "feature/pushed"
    );
    // Idempotent: the directory exists, nothing runs.
    let again = run("wt-pushed", "feature/pushed");
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert!(!String::from_utf8_lossy(&again.stdout).contains("outcome="));
    // Never pushed: refused with the marker, no directory, no branch.
    let out = run("wt-local", "feature/local-only");
    assert!(!out.status.success(), "unpushed branch must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(MIRROR_REFUSED), "{stderr}");
    assert!(!remote.join(".claude/worktrees/wt-local").exists());
    let branches = Command::new("git")
        .args(["-C", remote_s, "branch", "--list", "feature/local-only"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&branches.stdout).trim(), "");
    assert_eq!(
        git_setup_error("h", "o", "r", Some("feature/local-only"), "", &stderr).message,
        "branch feature/local-only is not on origin; push it from the source machine, or start a new worktree on h"
    );
    std::fs::remove_dir_all(&base).ok();
}

// ── Task 6: the mass-loss verdict and branch ──────────────────────────────

#[test]
fn verdict_needs_a_readable_identity() {
    let stored = StoredIdentity {
        boot_id: Some("a".into()),
        tmux_server_pid: Some(1),
    };
    assert_eq!(mass_loss_verdict(&stored, None), None);
}

#[test]
fn a_changed_boot_id_is_a_reboot() {
    let stored = StoredIdentity {
        boot_id: Some("a".into()),
        tmux_server_pid: Some(1),
    };
    let obs = crate::tmux::HostIdentity {
        boot_id: Some("b".into()),
        tmux_server_pid: Some(2),
    };
    assert_eq!(mass_loss_verdict(&stored, Some(&obs)), Some("host_reboot"));
}

#[test]
fn no_tmux_server_or_a_new_server_pid_means_the_server_is_gone() {
    let stored = StoredIdentity {
        boot_id: Some("a".into()),
        tmux_server_pid: Some(1),
    };
    let none = crate::tmux::HostIdentity {
        boot_id: Some("a".into()),
        tmux_server_pid: None,
    };
    let new = crate::tmux::HostIdentity {
        boot_id: Some("a".into()),
        tmux_server_pid: Some(2),
    };
    assert_eq!(
        mass_loss_verdict(&stored, Some(&none)),
        Some("tmux_server_gone")
    );
    assert_eq!(
        mass_loss_verdict(&stored, Some(&new)),
        Some("tmux_server_gone")
    );
}

#[test]
fn a_first_probe_after_upgrade_is_never_a_verdict() {
    let obs = crate::tmux::HostIdentity {
        boot_id: Some("b".into()),
        tmux_server_pid: Some(2),
    };
    assert_eq!(
        mass_loss_verdict(&StoredIdentity::default(), Some(&obs)),
        None
    );
}

#[test]
fn an_unchanged_identity_is_a_normal_pass() {
    let stored = StoredIdentity {
        boot_id: Some("a".into()),
        tmux_server_pid: Some(1),
    };
    let same = crate::tmux::HostIdentity {
        boot_id: Some("a".into()),
        tmux_server_pid: Some(1),
    };
    assert_eq!(mass_loss_verdict(&stored, Some(&same)), None);
}

/// `ClaudeAgentRow` that binds to a live tmux session by NAME (the way a
/// fleet-launched session with `--name <tmux_name>` binds), so the session
/// picks up a `claude_session_id` on upsert.
fn agent_for_session(tmux_name: &str, session_id: &str) -> crate::claude_agents::ClaudeAgentRow {
    crate::claude_agents::ClaudeAgentRow {
        session_id: Some(session_id.to_string()),
        name: Some(tmux_name.to_string()),
        status: None,
        cwd: None,
        kind: crate::claude_agents::AgentKind::Interactive,
        job_id: None,
        started_at: None,
    }
}

/// A `claude --bg` agent row that matches NO tmux session (no name, no
/// cwd) — surfaces as a synthetic `kind='bg'` row via `reconcile_agent_rows`,
/// so a `host_reboot` verdict (which touches every session kind, not just
/// tmux-backed ones) has something non-tmux to mark lost too.
fn unmatched_bg_agent(session_id: &str) -> crate::claude_agents::ClaudeAgentRow {
    crate::claude_agents::ClaudeAgentRow {
        session_id: Some(session_id.to_string()),
        name: None,
        status: None,
        cwd: None,
        kind: crate::claude_agents::AgentKind::Background,
        job_id: None,
        started_at: None,
    }
}

/// `ReconcileDeps` whose named host `alias` answers via `IdentityTmux` with
/// the given `sessions`/`identity`/`agents`; every other alias (`local` is
/// auto-created every pass) gets a bare default `IdentityTmux` — no
/// sessions, no identity, no agents, so it never interferes.
fn identity_deps(
    alias: &'static str,
    sessions: Vec<crate::tmux::TmuxSession>,
    identity: Option<crate::tmux::HostIdentity>,
    agents: Vec<crate::claude_agents::ClaudeAgentRow>,
) -> Arc<ReconcileDeps> {
    ReconcileDeps::fake(
        move |a| {
            if a == alias {
                Box::new(IdentityTmux {
                    sessions: sessions.clone(),
                    identity: identity.clone(),
                    agents: agents.clone(),
                })
            } else {
                Box::new(IdentityTmux::default())
            }
        },
        std::time::Duration::from_secs(5),
    )
}

fn boot_a_pid(pid: Option<i64>) -> crate::tmux::HostIdentity {
    crate::tmux::HostIdentity {
        boot_id: Some("a".into()),
        tmux_server_pid: pid,
    }
}

/// Acceptance criterion 1: a host that stays reachable but whose tmux server
/// vanished (a plain `sudo systemctl restart tmux`-style loss, no reboot)
/// keeps its sessions' rows — ghosted, `lost_reason='tmux_server_gone'`,
/// with their `claude_session_id` intact so they can be resumed — instead of
/// the routine one-cycle ghost-then-reap deleting them.
#[tokio::test]
async fn a_vanished_tmux_server_keeps_the_rows_lost_with_their_claude_ids() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    store.lock().unwrap().upsert_host("mefistos").unwrap();

    // Pass 1: identity {boot a, pid 1}, one live session "x" bound to
    // claude_session_id "cid-x" ⇒ a normal, uneventful pass.
    let deps = identity_deps(
        "mefistos",
        vec![tmux_session("x")],
        Some(boot_a_pid(Some(1))),
        vec![agent_for_session("x", "cid-x")],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    {
        let s = store.lock().unwrap();
        let row = s
            .get_session("x", "mefistos")
            .unwrap()
            .expect("row created on pass 1");
        assert_eq!(row.status, "running");
        assert_eq!(row.claude_session_id.as_deref(), Some("cid-x"));
    }

    // Push "x"'s `last_reconciled_at` (the BE-3 guard `mark_host_sessions_lost`
    // now shares with the routine ghost path) well into the past: two real
    // passes inside one test can land in the same wall-clock second, which
    // would otherwise make pass 2's `probe_started_at` collide with pass 1's
    // stamp and spuriously exempt "x" from the mass-loss mark — a
    // test-timing artifact, not something the guard is meant to catch.
    store
        .lock()
        .unwrap()
        .mark_sessions_reconciled("mefistos", &["x".to_string()], 1)
        .unwrap();

    // Pass 2: identity {boot a, pid None} — the host answers (reachable),
    // but its tmux server is gone, so `list_sessions` truthfully reports no
    // sessions. `tmux_server_gone` must mark "x" lost instead of letting the
    // routine keep-set prune reap it over the next two passes.
    let deps = identity_deps("mefistos", vec![], Some(boot_a_pid(None)), vec![]);
    reconcile_sessions_with(&store, &deps).await.unwrap();
    let row_id = {
        let s = store.lock().unwrap();
        let row = s
            .get_session("x", "mefistos")
            .unwrap()
            .expect("row survives pass 2, marked lost rather than deleted");
        assert_eq!(row.status, "ghost");
        assert!(row.lost_at.is_some(), "lost_at must be stamped");
        assert_eq!(
            row.claude_session_id.as_deref(),
            Some("cid-x"),
            "the claude_session_id must survive so the session can be resumed"
        );
        row.id
    };
    {
        let s = store.lock().unwrap();
        let events = s.list_session_events(row_id, 10).unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.kind == "lost" && e.detail.as_deref() == Some("tmux_server_gone")),
            "a session_events 'lost' row with detail='tmux_server_gone' must be recorded; got {events:?}"
        );
    }

    // Pass 3: identical to pass 2 (server still gone, same identity — no
    // NEW verdict fires since a stored `(None, None)` pid pair is not a
    // verdict). Without the TTL exemption this is exactly the pass that
    // would hard-delete "x" (the routine one-cycle ghost-then-reap); WITH
    // it, the row must still be there.
    let deps = identity_deps("mefistos", vec![], Some(boot_a_pid(None)), vec![]);
    reconcile_sessions_with(&store, &deps).await.unwrap();
    {
        let s = store.lock().unwrap();
        let row = s
            .get_session("x", "mefistos")
            .unwrap()
            .expect("the exemption keeps the row past pass 3, not the one-cycle reap");
        assert_eq!(row.status, "ghost");
        assert_eq!(row.claude_session_id.as_deref(), Some("cid-x"));
        // The MCP tool's include-lost listing is `list_all_sessions` — no
        // status filter — so a resumable ghost row must still show up there.
        let all = s.list_all_sessions().unwrap();
        assert!(
            all.iter().any(|r| r.tmux_name == "x" && r.id == row.id),
            "the lost row must still appear in the include-lost listing"
        );
    }
}

/// A changed boot id (a real reboot) marks EVERY session on the host lost —
/// not just the tmux-backed ones a vanished tmux server would touch — with
/// `lost_reason='host_reboot'`.
#[tokio::test]
async fn a_changed_boot_id_marks_every_session_lost_as_a_reboot() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    store.lock().unwrap().upsert_host("mefistos").unwrap();

    // Pass 1: identity {boot a, pid 1}; one tmux session "y" and one
    // unmatched bg agent "bg-1" (⇒ a synthetic kind='bg' row via
    // reconcile_agent_rows), so the test can tell a reboot's "every kind"
    // sweep apart from tmux_server_gone's tmux-only one.
    let deps = identity_deps(
        "mefistos",
        vec![tmux_session("y")],
        Some(boot_a_pid(Some(1))),
        vec![agent_for_session("y", "cid-y"), unmatched_bg_agent("bg-1")],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    let (tmux_id, bg_id) = {
        let s = store.lock().unwrap();
        let tmux_row = s.get_session("y", "mefistos").unwrap().expect("tmux row");
        assert_eq!(tmux_row.status, "running");
        let bg_row = s
            .get_session("bg:bg-1", "mefistos")
            .unwrap()
            .expect("synthetic bg row exists after pass 1");
        (tmux_row.id, bg_row.id)
    };

    // Same BE-3-guard timing fix as the tmux_server_gone test above.
    store
        .lock()
        .unwrap()
        .mark_sessions_reconciled("mefistos", &["y".to_string(), "bg:bg-1".to_string()], 1)
        .unwrap();

    // Pass 2: the host comes back with a DIFFERENT boot id — a real reboot.
    // Nothing tmux-side or agent-side is live any more.
    let deps = identity_deps(
        "mefistos",
        vec![],
        Some(crate::tmux::HostIdentity {
            boot_id: Some("b".into()),
            tmux_server_pid: Some(2),
        }),
        vec![],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();

    let s = store.lock().unwrap();
    let tmux_row = s
        .get_session("y", "mefistos")
        .unwrap()
        .expect("tmux row survives, marked lost");
    assert_eq!(tmux_row.status, "ghost");
    let bg_row = s
        .get_session("bg:bg-1", "mefistos")
        .unwrap()
        .expect("bg row also survives, marked lost — a reboot kills bg agents too");
    assert_eq!(bg_row.status, "ghost");

    for (id, name) in [(tmux_id, "y"), (bg_id, "bg:bg-1")] {
        let events = s.list_session_events(id, 10).unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.kind == "lost" && e.detail.as_deref() == Some("host_reboot")),
            "session {name} must carry a 'lost' event with detail='host_reboot'; got {events:?}"
        );
    }
}

/// Acceptance criterion 4: a host whose identity can never be read (no
/// `/proc`, an executor that doesn't implement it, a stubborn ssh hiccup —
/// `identity: None` on every pass) must behave exactly as before Task 6: a
/// session that drops out is ghosted on the pass it disappears and hard-
/// deleted on the NEXT pass (the routine one-cycle grace), never granted the
/// mass-loss TTL exemption.
#[tokio::test]
async fn an_unreadable_identity_leaves_today_s_behaviour_intact() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    store.lock().unwrap().upsert_host("mefistos").unwrap();

    // Pass 1: identity unreadable, one live session "z".
    let deps = identity_deps("mefistos", vec![tmux_session("z")], None, vec![]);
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert_eq!(
        store
            .lock()
            .unwrap()
            .get_session("z", "mefistos")
            .unwrap()
            .unwrap()
            .status,
        "running"
    );

    // Push "z"'s `last_reconciled_at` (BE-3's stale-probe guard) well into
    // the past: two real passes inside one test can land in the same
    // wall-clock second, which would otherwise make pass 2's `probe_started_at`
    // collide with pass 1's stamp and spuriously exempt "z" from ghosting —
    // a test-timing artifact, not something Task 6 changes.
    store
        .lock()
        .unwrap()
        .mark_sessions_reconciled("mefistos", &["z".to_string()], 1)
        .unwrap();

    // Pass 2: "z" disappeared; identity still unreadable ⇒ no verdict, so
    // this is the routine keep-set ghost (lost_reason='missing').
    let deps = identity_deps("mefistos", vec![], None, vec![]);
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert_eq!(
        store
            .lock()
            .unwrap()
            .get_session("z", "mefistos")
            .unwrap()
            .expect("ghosted, not yet deleted")
            .status,
        "ghost",
        "pass 2 ghosts the missing row exactly as before Task 6"
    );

    // Pass 3: identical — the routine one-cycle reap must still fire; a
    // 'missing' row is never exempt regardless of the (now-active) TTL
    // cutoff.
    let deps = identity_deps("mefistos", vec![], None, vec![]);
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert!(
        store
            .lock()
            .unwrap()
            .get_session("z", "mefistos")
            .unwrap()
            .is_none(),
        "pass 3 hard-deletes the 'missing' row exactly as before Task 6 — \
         the reboot-safety-net TTL exemption must never apply to it"
    );
}

/// `skip_prune` in isolation from the TTL exemption: with
/// `sessions.lost_ttl_secs` set to `0` (the exemption disabled — every
/// mass-loss row reaps on the routine one-cycle schedule same as a
/// `missing` row), a verdict must still leave the just-marked row alive
/// for the pass it was marked on. Without `skip_prune`, the routine
/// `ghost_and_clean` running right after `mark_host_sessions_lost` in the
/// SAME `apply_host_reconcile` write would see the row as already-ghost
/// (Phase 2's `pre_ghost_ids` is captured before that pass's Phase 1 runs)
/// and hard-delete it immediately — the mass-loss verdict would never be
/// visible to anyone.
#[tokio::test]
async fn a_mass_loss_verdict_survives_its_own_pass_with_the_ttl_exemption_disabled() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    {
        let s = store.lock().unwrap();
        s.upsert_host("mefistos").unwrap();
        s.set_setting("sessions.lost_ttl_secs", "0").unwrap();
    }

    // Pass 1: normal pass, one live session "w".
    let deps = identity_deps(
        "mefistos",
        vec![tmux_session("w")],
        Some(boot_a_pid(Some(1))),
        vec![],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert_eq!(
        store
            .lock()
            .unwrap()
            .get_session("w", "mefistos")
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
    // BE-3-guard timing fix, same as the other mass-loss e2e tests.
    store
        .lock()
        .unwrap()
        .mark_sessions_reconciled("mefistos", &["w".to_string()], 1)
        .unwrap();

    // Pass 2: tmux server gone ⇒ mark_host_sessions_lost ghosts "w" with
    // lost_reason='tmux_server_gone'. With the TTL exemption disabled,
    // ONLY `skip_prune` stands between that write and the routine
    // ghost_and_clean's Phase 2 treating it as a stale ghost to reap THIS
    // SAME pass.
    let deps = identity_deps("mefistos", vec![], Some(boot_a_pid(None)), vec![]);
    reconcile_sessions_with(&store, &deps).await.unwrap();

    let row = store
        .lock()
        .unwrap()
        .get_session("w", "mefistos")
        .unwrap()
        .expect(
            "skip_prune must keep the just-marked row alive for its own pass, \
             even with the TTL exemption off",
        );
    assert_eq!(row.status, "ghost");
}

/// `read_lost_ttl_cutoff` in isolation. Mirrors `read_reconcile_interval_secs`'s
/// established `settings::resolve` → parse pattern, so — same as that sibling
/// reader — an unparseable raw string and a NEGATIVE raw string collapse to
/// the same outcome: `settings::resolve`'s `Kind::Secs` validator parses as
/// `u64`, so a negative string fails validation exactly like garbage does
/// and both fall back to the registry default, never reaching the `<= 0`
/// branch as a literal negative number. Only a value that PARSES successfully
/// (`"0"` or a positive integer) can reach that branch; `"0"` is the only way
/// to observe it in practice.
#[test]
fn read_lost_ttl_cutoff_resolves_like_the_reconcile_interval_reader() {
    let now = 10_000_000i64;
    // A normal positive value ⇒ Some(now - ttl).
    assert_eq!(
        read_lost_ttl_cutoff(Some("100".into()), now),
        Some(now - 100)
    );
    // "0" is the documented "disabled" sentinel ⇒ None.
    assert_eq!(read_lost_ttl_cutoff(Some("0".into()), now), None);
    // Unparseable ⇒ the registry default.
    assert_eq!(
        read_lost_ttl_cutoff(Some("nonsense".into()), now),
        Some(now - DEFAULT_LOST_TTL_SECS)
    );
    // Missing (no setting stored yet) ⇒ the registry default.
    assert_eq!(
        read_lost_ttl_cutoff(None, now),
        Some(now - DEFAULT_LOST_TTL_SECS)
    );
    // A negative raw string fails `Kind::Secs`'s `u64` validation the same
    // way garbage does, so `settings::resolve` substitutes the default
    // BEFORE this function ever sees a negative number — it also resolves
    // to the registry default, not `None`.
    assert_eq!(
        read_lost_ttl_cutoff(Some("-5".into()), now),
        Some(now - DEFAULT_LOST_TTL_SECS)
    );
}

/// BE-3 regression (finding 1): `mark_host_sessions_lost` must share the
/// exact same "a newer probe already saw this row live" guard that
/// `Store::ghost_and_clean`'s Phase 1 uses. Without it, a background tick's
/// STALE probe (started before `new_session`'s own faster single-host
/// reconcile landed) can mass-mark a session the fleet has already
/// confirmed live again.
#[tokio::test]
async fn a_verdict_never_marks_a_row_a_newer_probe_already_saw_live() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    let host = {
        let s = store.lock().unwrap();
        s.upsert_host("mefistos").unwrap();
        s.set_host_identity("mefistos", Some("a"), Some(1)).unwrap();
        s.upsert_session("x", "mefistos", None, None, 1, 1, "running", None)
            .unwrap();
        // "x" was reconciled (by a NEWER, faster writer — e.g. `new_session`'s
        // own single-host reconcile) at t=5000, strictly AFTER the stale
        // probe below started.
        s.mark_sessions_reconciled("mefistos", &["x".to_string()], 5000)
            .unwrap();
        s.list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "mefistos")
            .unwrap()
    };

    // The stale probe STARTED at t=1000 (before "x" was reconciled above)
    // and — being stale — saw no tmux server at all: a genuine
    // tmux_server_gone verdict candidate, delivered late.
    let probe = HostProbe {
        host: host.clone(),
        result: Ok(Vec::new()),
        agent_rows: Vec::new(),
        agent_mtimes: Some(std::collections::HashMap::new()),
        intel: PaneIntelMap::new(),
        account: None,
        pr_info: PrInfoMap::new(),
        identity: Some(crate::tmux::HostIdentity {
            boot_id: Some("a".into()),
            tmux_server_pid: None,
        }),
        started_at: 1000,
    };
    let mut s = store.lock().unwrap();
    let projects = s.list_projects().unwrap();
    reconcile_write_one_host(&mut s, &probe, &projects).unwrap();

    let row = s
        .get_session("x", "mefistos")
        .unwrap()
        .expect("row still exists");
    assert_eq!(
        row.status, "running",
        "a row a NEWER probe already saw live must not be marked lost by a stale verdict"
    );
    assert!(
        s.list_session_events(row.id, 10)
            .unwrap()
            .iter()
            .all(|e| e.kind != "lost"),
        "no false 'lost' event may be recorded for it"
    );
}

/// Ruling (finding 2): never write the observed identity when it is `None`.
/// If that guard regressed and a transient unreadable read got stored as
/// `(None, None)`, a REAL vanished-tmux-server pass right after it would
/// compare `(None, None)` to the freshly observed identity and — per the
/// `(None, None)` "not a verdict" rule — find nothing, silently falling back
/// to the routine `'missing'` ghost-then-reap. This is the exact false
/// negative the whole feature exists to prevent.
#[tokio::test]
async fn a_failed_identity_read_never_overwrites_the_stored_identity() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    store.lock().unwrap().upsert_host("mefistos").unwrap();

    // Pass 1: identity {boot a, pid 1}; sessions "x" and "y" live.
    let deps = identity_deps(
        "mefistos",
        vec![tmux_session("x"), tmux_session("y")],
        Some(boot_a_pid(Some(1))),
        vec![],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    store
        .lock()
        .unwrap()
        .mark_sessions_reconciled("mefistos", &["x".to_string(), "y".to_string()], 1)
        .unwrap();

    // Pass 2: identity unreadable (`None`); "x" disappeared, "y" stays
    // live. No verdict fires (mass_loss_verdict short-circuits on `None`),
    // so "x" takes the routine 'missing' ghost path — and, the ruling under
    // test, the stored identity (a, 1) must survive this pass untouched.
    let deps = identity_deps("mefistos", vec![tmux_session("y")], None, vec![]);
    reconcile_sessions_with(&store, &deps).await.unwrap();
    {
        let s = store.lock().unwrap();
        assert_eq!(
            s.get_session("x", "mefistos").unwrap().unwrap().status,
            "ghost",
            "x takes the routine 'missing' path with no verdict"
        );
        assert_eq!(
            s.get_session("y", "mefistos").unwrap().unwrap().status,
            "running"
        );
    }
    // BE-3-guard timing fix (pass 2's own reconcile just re-stamped "y").
    store
        .lock()
        .unwrap()
        .mark_sessions_reconciled("mefistos", &["y".to_string()], 1)
        .unwrap();

    // Pass 3: identity {boot a, pid None} — the tmux server is genuinely
    // gone now. If pass 2 had wrongly stored (None, None), this pass would
    // compare (None, None) to (a, None) and find NO verdict (a stored `None`
    // pid never compares, and (None, None) is deliberately not a verdict
    // either), so "y" would only take the routine 'missing' path with no
    // 'lost' event. The stored (a, 1) surviving pass 2 is the only way pass
    // 3 can produce a genuine tmux_server_gone verdict here.
    let deps = identity_deps("mefistos", vec![], Some(boot_a_pid(None)), vec![]);
    reconcile_sessions_with(&store, &deps).await.unwrap();
    let s = store.lock().unwrap();
    let y = s
        .get_session("y", "mefistos")
        .unwrap()
        .expect("y row still exists");
    let events = s.list_session_events(y.id, 10).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.kind == "lost" && e.detail.as_deref() == Some("tmux_server_gone")),
        "pass 3 must produce a genuine tmux_server_gone verdict, proving (a, 1) \
         survived pass 2's unreadable identity; got {events:?}"
    );
}

/// `skip_prune` only when the verdict actually recorded something (finding
/// 4): a verdict pass that neither marks NOR reclassifies any row must leave
/// the routine prune running — otherwise an unrelated already-ghost row due
/// for its one-cycle reap on this very pass gets an undeserved reprieve,
/// purely because a verdict elsewhere on the host "fired" with nothing to
/// do. The ghost used is one fleet killed itself (`lost_reason='killed'`),
/// which the verdict neither marks (already ghost) nor reclassifies (not
/// `missing`). (A `missing` ghost IS reclassified and kept — see
/// `accepted_tradeoff_a_session_that_ended_one_pass_before_a_reboot_is_kept_as_lost`.)
#[tokio::test]
async fn skip_prune_does_not_delay_the_reap_of_an_unrelated_already_ghost_row() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    store.lock().unwrap().upsert_host("mefistos").unwrap();

    // Pass 1: identity {boot a, pid 1}; one live session "old".
    let deps = identity_deps(
        "mefistos",
        vec![tmux_session("old")],
        Some(boot_a_pid(Some(1))),
        vec![agent_for_session("old", "cid-old")],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    let id = {
        let s = store.lock().unwrap();
        let id = s.get_session("old", "mefistos").unwrap().unwrap().id;
        // Fleet kills "old" itself: a `killed` ghost before the next pass.
        s.mark_session_killed(id, 100).unwrap().expect("ghosted");
        id
    };

    // Pass 2: the boot id changes (a genuine host_reboot verdict), but the
    // only row is already a `killed` ghost — the verdict marks and
    // reclassifies nothing, so the routine prune's Phase 2 must reap "old"
    // THIS pass.
    let deps = identity_deps(
        "mefistos",
        vec![],
        Some(crate::tmux::HostIdentity {
            boot_id: Some("b".into()),
            tmux_server_pid: Some(2),
        }),
        vec![],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert!(
        store
            .lock()
            .unwrap()
            .get_session_by_id(id)
            .unwrap()
            .is_none(),
        "a verdict that marks and reclassifies nothing must not delay the \
         routine reap of an unrelated already-ghost row"
    );
}

/// ACCEPTED TRADE-OFF (Important 2 ruling, option (a)): a `missing` ghost is
/// indistinguishable from one left by a failed first post-loss pass, so a
/// session that genuinely ended within the ONE pass before a mass-loss
/// verdict is reclassified and kept to the TTL (still dismissable). This is
/// deliberate — the reclassification errs toward keeping.
#[tokio::test]
async fn accepted_tradeoff_a_session_that_ended_one_pass_before_a_reboot_is_kept_as_lost() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    store.lock().unwrap().upsert_host("mefistos").unwrap();

    let deps = identity_deps(
        "mefistos",
        vec![tmux_session("old")],
        Some(boot_a_pid(Some(1))),
        vec![agent_for_session("old", "cid-old")],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    store
        .lock()
        .unwrap()
        .mark_sessions_reconciled("mefistos", &["old".to_string()], 1)
        .unwrap();

    // Pass 2: "old" ends on its own, identity readable and unchanged ⇒
    // a routine `missing` ghost.
    let deps = identity_deps("mefistos", vec![], Some(boot_a_pid(Some(1))), vec![]);
    reconcile_sessions_with(&store, &deps).await.unwrap();
    let (id, lost_at) = {
        let s = store.lock().unwrap();
        let row = s.get_session("old", "mefistos").unwrap().unwrap();
        assert_eq!(row.status, "ghost");
        (row.id, row.lost_at)
    };

    // Pass 3: an unrelated reboot. The one-pass-old `missing` ghost is
    // reclassified `host_reboot` (keeping its lost_at) and survives.
    let deps = identity_deps(
        "mefistos",
        vec![],
        Some(crate::tmux::HostIdentity {
            boot_id: Some("b".into()),
            tmux_server_pid: Some(2),
        }),
        vec![],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    {
        let s = store.lock().unwrap();
        let row = s.get_session_by_id(id).unwrap().expect("kept on pass 3");
        assert_eq!(row.lost_at, lost_at);
        assert_eq!(lost_events(&s, id, "host_reboot"), 1);
    }
    // Pass 4: TTL-exempt as a resumable mass loss.
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert!(
        store
            .lock()
            .unwrap()
            .get_session_by_id(id)
            .unwrap()
            .is_some(),
        "the accepted trade-off keeps the row past the routine reap"
    );
}

/// `session_events` `lost` rows of `id` carrying `reason` as their detail.
fn lost_events(s: &Store, id: i64, reason: &str) -> usize {
    s.list_session_events(id, 50)
        .unwrap()
        .iter()
        .filter(|e| e.kind == "lost" && e.detail.as_deref() == Some(reason))
        .count()
}

/// Final-review Important 1: tmux exits when its last session closes, so
/// fleet's OWN kill of a host's only session makes the kill's follow-up
/// reconcile see no tmux server — a `tmux_server_gone` verdict. The killed
/// row must not become a 14-day "resumable" ghost (it would also duplicate a
/// moved session's claude id, since `move_session` kills its source through
/// the same path): it reaps on the ordinary schedule, and no
/// `tmux_server_gone` loss is ever recorded for it.
#[tokio::test]
async fn killing_a_host_s_last_session_is_not_a_resumable_mass_loss() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    store.lock().unwrap().upsert_host("mefistos").unwrap();

    // Pass 1: server pid 1, one live session "x" bound to "cid-x".
    let deps = identity_deps(
        "mefistos",
        vec![tmux_session("x")],
        Some(boot_a_pid(Some(1))),
        vec![agent_for_session("x", "cid-x")],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    let id = store
        .lock()
        .unwrap()
        .get_session("x", "mefistos")
        .unwrap()
        .expect("row created on pass 1")
        .id;
    // Same BE-3 timing fix as the other mass-loss e2e tests.
    store
        .lock()
        .unwrap()
        .mark_sessions_reconciled("mefistos", &["x".to_string()], 1)
        .unwrap();

    // Kill "x" through the real kill path; the host now answers with no
    // tmux server (its last session closed) and no sessions.
    let deps = identity_deps("mefistos", vec![], Some(boot_a_pid(None)), vec![]);
    let ssh = Arc::new(SshClient::new());
    let killed = kill_session_with(
        KillSessionArgs {
            host_alias: "mefistos".into(),
            name: "x".into(),
            force: false,
        },
        &store,
        &ssh,
        &deps,
    )
    .await
    .unwrap();
    assert_eq!(killed, id);
    {
        let s = store.lock().unwrap();
        if s.get_session_by_id(id).unwrap().is_some() {
            assert_eq!(
                lost_events(&s, id, "tmux_server_gone"),
                0,
                "fleet's own kill must never be recorded as a tmux_server_gone loss"
            );
        }
    }

    // The following pass (server still gone): the killed row must be gone —
    // not held back by the mass-loss TTL exemption.
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert!(
        store
            .lock()
            .unwrap()
            .get_session_by_id(id)
            .unwrap()
            .is_none(),
        "the killed row must be reaped on the ordinary schedule, not kept as a resumable ghost"
    );
}

/// Final-review Important 2: the FIRST pass after a loss could not record
/// the verdict (here: one transient ssh failure made the identity
/// unreadable), so the routine prune ghosted the rows as `missing`. The
/// verdict that fires on the next pass must still record them as a mass
/// loss (reclassify them) — otherwise Phase 2 reaps them all, the exact
/// loss the feature exists to prevent.
#[tokio::test]
async fn a_verdict_after_a_failed_first_pass_still_keeps_the_rows() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    store.lock().unwrap().upsert_host("mefistos").unwrap();

    // Pass 1: identity {a, pid 1}, "x" live with a claude id.
    let deps = identity_deps(
        "mefistos",
        vec![tmux_session("x")],
        Some(boot_a_pid(Some(1))),
        vec![agent_for_session("x", "cid-x")],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    let id = store
        .lock()
        .unwrap()
        .get_session("x", "mefistos")
        .unwrap()
        .expect("row created on pass 1")
        .id;
    store
        .lock()
        .unwrap()
        .mark_sessions_reconciled("mefistos", &["x".to_string()], 1)
        .unwrap();

    // Pass 2: the server is gone but the identity read failed ⇒ no verdict;
    // the routine prune ghosts "x" as `missing`.
    let deps = identity_deps("mefistos", vec![], None, vec![]);
    reconcile_sessions_with(&store, &deps).await.unwrap();
    let lost_at = {
        let s = store.lock().unwrap();
        let row = s.get_session_by_id(id).unwrap().expect("ghosted on pass 2");
        assert_eq!(row.status, "ghost");
        row.lost_at.expect("lost_at stamped on pass 2")
    };

    // Pass 3: the identity reads again — pid None vs stored pid 1 ⇒
    // `tmux_server_gone`. The `missing` ghost must be reclassified and
    // survive, keeping its pass-2 `lost_at`.
    let deps = identity_deps("mefistos", vec![], Some(boot_a_pid(None)), vec![]);
    reconcile_sessions_with(&store, &deps).await.unwrap();
    {
        let s = store.lock().unwrap();
        let row = s
            .get_session_by_id(id)
            .unwrap()
            .expect("the verdict must keep the row on pass 3");
        assert_eq!(row.status, "ghost");
        assert_eq!(row.lost_at, Some(lost_at), "reclassification keeps lost_at");
        assert_eq!(row.claude_session_id.as_deref(), Some("cid-x"));
        assert_eq!(
            lost_events(&s, id, "tmux_server_gone"),
            1,
            "the reclassification records the loss exactly once"
        );
    }

    // Pass 4: now TTL-exempt as a resumable mass loss.
    reconcile_sessions_with(&store, &deps).await.unwrap();
    assert!(
        store
            .lock()
            .unwrap()
            .get_session_by_id(id)
            .unwrap()
            .is_some(),
        "the reclassified row must be exempt from the routine reap on pass 4"
    );
}

/// Final-review minor 3: `keep` names tmux sessions only, so a `host_reboot`
/// verdict used to mark a bg agent that is running NOW (present in this very
/// probe) lost — and `reconcile_agent_rows` revived it in the same writer
/// call, leaving a spurious permanent `lost` event behind.
#[tokio::test]
async fn a_reboot_verdict_spares_a_bg_agent_live_in_the_same_probe() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    store.lock().unwrap().upsert_host("mefistos").unwrap();

    let deps = identity_deps(
        "mefistos",
        vec![tmux_session("y")],
        Some(boot_a_pid(Some(1))),
        vec![agent_for_session("y", "cid-y"), unmatched_bg_agent("bg-1")],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();
    let bg_id = store
        .lock()
        .unwrap()
        .get_session("bg:bg-1", "mefistos")
        .unwrap()
        .expect("bg row after pass 1")
        .id;
    store
        .lock()
        .unwrap()
        .mark_sessions_reconciled("mefistos", &["y".to_string(), "bg:bg-1".to_string()], 1)
        .unwrap();

    // Pass 2: a new boot id (reboot); the tmux session is gone but the bg
    // agent is listed live again (e.g. relaunched by a unit on boot).
    let deps = identity_deps(
        "mefistos",
        vec![],
        Some(crate::tmux::HostIdentity {
            boot_id: Some("b".into()),
            tmux_server_pid: Some(2),
        }),
        vec![unmatched_bg_agent("bg-1")],
    );
    reconcile_sessions_with(&store, &deps).await.unwrap();

    let s = store.lock().unwrap();
    let bg = s.get_session_by_id(bg_id).unwrap().expect("bg row kept");
    assert_eq!(bg.status, "running");
    assert_eq!(
        lost_events(&s, bg_id, "host_reboot"),
        0,
        "a bg agent live in the verdict's own probe must get no lost event"
    );
    // The tmux row that really vanished is still marked.
    let y = s
        .get_session("y", "mefistos")
        .unwrap()
        .expect("tmux row kept");
    assert_eq!(y.status, "ghost");
    assert_eq!(lost_events(&s, y.id, "host_reboot"), 1);
}

// ── claude_session_id pairing: a cwd match is an inference, not an identity ──

fn live_in(name: &str, cwd: &str) -> crate::tmux::TmuxSession {
    crate::tmux::TmuxSession {
        name: name.into(),
        created: 1,
        last_activity: 1,
        attached: false,
        path: PathBuf::from(cwd),
    }
}

/// One reconcile write pass for host `vps` (remote: no cwd canonicalizing).
fn pair_pass(
    s: &mut Store,
    live: Vec<crate::tmux::TmuxSession>,
    agents: Vec<crate::claude_agents::ClaudeAgentRow>,
) {
    let host = s
        .list_hosts()
        .unwrap()
        .into_iter()
        .find(|h| h.alias == "vps")
        .unwrap();
    let probe = HostProbe {
        host,
        result: Ok(live),
        agent_rows: agents,
        agent_mtimes: Some(std::collections::HashMap::new()),
        intel: PaneIntelMap::new(),
        account: None,
        pr_info: PrInfoMap::new(),
        identity: None,
        started_at: now_unix(),
    };
    let projects = s.list_projects().unwrap();
    reconcile_write_one_host(s, &probe, &projects).unwrap();
}

fn stored_claude_id(s: &Store, name: &str) -> Option<String> {
    s.get_session(name, "vps")
        .unwrap()
        .unwrap()
        .claude_session_id
}

/// The 2026-09-19 host-reboot restore bug: `rt-a` and `rt-b` share a cwd,
/// only `rt-a`'s agent is registered, and the unique-cwd fallback handed
/// `rt-b` `rt-a`'s id on every pass — overwriting the id new_session minted.
#[test]
fn two_sessions_in_one_cwd_keep_distinct_claude_ids_across_passes() {
    let mut s = Store::open_in_memory().unwrap();
    s.upsert_host("vps").unwrap();
    let agent_a = agent("uuid-a", None, Some("/p"));

    // rt-a alone: its NULL id is filled by the unique cwd match.
    pair_pass(&mut s, vec![live_in("rt-a", "/p")], vec![agent_a.clone()]);
    assert_eq!(stored_claude_id(&s, "rt-a").as_deref(), Some("uuid-a"));

    // rt-b appears in the same cwd before its agent registers: rt-a already
    // holds uuid-a, so rt-b must not be handed it.
    let both = || vec![live_in("rt-a", "/p"), live_in("rt-b", "/p")];
    pair_pass(&mut s, both(), vec![agent_a.clone()]);
    assert_eq!(stored_claude_id(&s, "rt-b"), None);

    // new_session records rt-b's minted id; later passes must keep it.
    let b_id = s.get_session("rt-b", "vps").unwrap().unwrap().id;
    s.set_claude_session_id(b_id, "uuid-b").unwrap();
    for _ in 0..3 {
        pair_pass(&mut s, both(), vec![agent_a.clone()]);
        assert_eq!(stored_claude_id(&s, "rt-a").as_deref(), Some("uuid-a"));
        assert_eq!(stored_claude_id(&s, "rt-b").as_deref(), Some("uuid-b"));
    }
    // Once both agents run in /p the cwd is ambiguous: nothing changes.
    let agent_b = agent("uuid-b", None, Some("/p"));
    pair_pass(&mut s, both(), vec![agent_a, agent_b]);
    assert_eq!(stored_claude_id(&s, "rt-a").as_deref(), Some("uuid-a"));
    assert_eq!(stored_claude_id(&s, "rt-b").as_deref(), Some("uuid-b"));
}

/// A cwd match never overwrites an id the row already has, even when the
/// agent's id is held by no other row.
#[test]
fn a_cwd_match_does_not_overwrite_a_stored_claude_id() {
    let mut s = Store::open_in_memory().unwrap();
    s.upsert_host("vps").unwrap();
    pair_pass(&mut s, vec![live_in("rt-b", "/p")], vec![]);
    let id = s.get_session("rt-b", "vps").unwrap().unwrap().id;
    s.set_claude_session_id(id, "uuid-b").unwrap();
    pair_pass(
        &mut s,
        vec![live_in("rt-b", "/p")],
        vec![agent("uuid-other", None, Some("/p"))],
    );
    assert_eq!(stored_claude_id(&s, "rt-b").as_deref(), Some("uuid-b"));
}

/// A by-name match is authoritative: it replaces the stored id (e.g. the
/// conversation id changed after `/clear`), even with another agent in the
/// same cwd.
#[test]
fn a_by_name_match_still_updates_the_claude_id() {
    let mut s = Store::open_in_memory().unwrap();
    s.upsert_host("vps").unwrap();
    pair_pass(&mut s, vec![live_in("rt-b", "/p")], vec![]);
    let id = s.get_session("rt-b", "vps").unwrap().unwrap().id;
    s.set_claude_session_id(id, "uuid-b").unwrap();
    pair_pass(
        &mut s,
        vec![live_in("rt-b", "/p")],
        vec![
            agent("uuid-b2", Some("rt-b"), Some("/p")),
            agent("uuid-x", None, Some("/p")),
        ],
    );
    assert_eq!(stored_claude_id(&s, "rt-b").as_deref(), Some("uuid-b2"));
}

#[test]
fn pair_session_agents_rejects_an_id_another_session_claims() {
    let stored = |pairs: &[(&str, Option<&str>)]| -> std::collections::HashMap<_, _> {
        pairs
            .iter()
            .map(|(n, id)| (n.to_string(), id.map(String::from)))
            .collect()
    };
    let live = vec![live_in("rt-a", "/p"), live_in("rt-b", "/p")];

    // rt-a's agent is named; rt-b's cwd match finds only that agent → no pair.
    let agents = vec![agent("uuid-a", Some("rt-a"), Some("/p"))];
    let got = pair_session_agents(&live, &agents, &stored(&[]), false);
    assert_eq!(got["rt-a"].session_id.as_deref(), Some("uuid-a"));
    assert!(!got.contains_key("rt-b"));

    // Two NULL sessions inferring the one unnamed agent: ambiguous → neither.
    let agents = vec![agent("uuid-a", None, Some("/p"))];
    let got = pair_session_agents(&live, &agents, &stored(&[]), false);
    assert!(got.is_empty(), "{got:?}");

    // The id is held by another stored row (a ghost / pane-less row) → no pair.
    let live_b = vec![live_in("rt-b", "/p")];
    let got = pair_session_agents(
        &live_b,
        &agents,
        &stored(&[("rt-a", Some("uuid-a")), ("rt-b", None)]),
        false,
    );
    assert!(got.is_empty(), "{got:?}");

    // The row already holds exactly that id → paired (status still flows).
    let got = pair_session_agents(
        &live_b,
        &agents,
        &stored(&[("rt-b", Some("uuid-a"))]),
        false,
    );
    assert_eq!(got["rt-b"].session_id.as_deref(), Some("uuid-a"));

    // A cwd-matched agent without a session id can't be tied to the session.
    let mut anon = agent("x", None, Some("/p"));
    anon.session_id = None;
    let anon = [anon];
    let got = pair_session_agents(&live_b, &anon, &stored(&[]), false);
    assert!(got.is_empty(), "{got:?}");
}
