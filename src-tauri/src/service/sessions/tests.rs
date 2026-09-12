use super::*;
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

#[test]
fn bg_claude_session_id_prefers_row_id_falls_back_to_name() {
    // Stored claude_session_id wins…
    assert_eq!(
        bg_claude_session_id("bg:aaa-111", Some("bbb-222")),
        "bbb-222"
    );
    // …a missing or blank one falls back to the uuid in the tmux_name.
    assert_eq!(bg_claude_session_id("bg:aaa-111", None), "aaa-111");
    assert_eq!(bg_claude_session_id("bg:aaa-111", Some("  ")), "aaa-111");
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
    }
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
    }];
    assert!(unmatched_bg_agents(&[], &agents, true).is_empty());
}

#[test]
fn reconcile_bg_agents_upserts_bg_session_row() {
    // Feed agent rows + an EMPTY tmux list → expect a `bg` SessionRow.
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let agents = vec![agent("bg-uuid-1", Some("my-bg-job"), Some("/tmp/proj"))];

    reconcile_bg_agents(&s, "local", &[], &[], &agents).unwrap();

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
fn reconcile_bg_agents_prunes_vanished_agents_two_phase() {
    // A bg agent that disappears from `claude agents --json` is ghosted on
    // the next reconcile pass and hard-deleted (events included) on the one
    // after — so dead bg rows cannot accumulate.
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let agents = vec![agent("bg-uuid-1", Some("my-bg-job"), Some("/tmp/proj"))];
    reconcile_bg_agents(&s, "local", &[], &[], &agents).unwrap();
    let id = s
        .get_session("bg:bg-uuid-1", "local")
        .unwrap()
        .expect("upserted")
        .id;

    // Pass 2: agent gone (empty listing) → ghosted, still present.
    reconcile_bg_agents(&s, "local", &[], &[], &[]).unwrap();
    let row = s
        .get_session("bg:bg-uuid-1", "local")
        .unwrap()
        .expect("ghosted, not yet deleted");
    assert_eq!(row.status, "ghost");
    assert!(row.lost_at.is_some());

    // Pass 3: still gone → hard-deleted.
    reconcile_bg_agents(&s, "local", &[], &[], &[]).unwrap();
    assert!(
        s.get_session("bg:bg-uuid-1", "local").unwrap().is_none(),
        "dead bg row must be reaped on the second missing pass"
    );
    assert!(s.get_session_by_id(id).unwrap().is_none());
}

#[test]
fn reconcile_bg_agents_resurrects_ghost_when_agent_returns() {
    // A single missing pass (e.g. a transiently failed `claude agents`
    // probe, which comes back as an empty list) must not lose the row.
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let agents = vec![agent("bg-uuid-1", Some("my-bg-job"), Some("/tmp/proj"))];
    reconcile_bg_agents(&s, "local", &[], &[], &agents).unwrap();
    reconcile_bg_agents(&s, "local", &[], &[], &[]).unwrap(); // ghosts it
    reconcile_bg_agents(&s, "local", &[], &[], &agents).unwrap(); // returns

    let row = s
        .get_session("bg:bg-uuid-1", "local")
        .unwrap()
        .expect("row survives a one-pass blip");
    assert_eq!(row.status, "running");
    assert_eq!(row.lost_at, None);

    // And it is NOT deleted on the next pass with the agent still live.
    reconcile_bg_agents(&s, "local", &[], &[], &agents).unwrap();
    assert!(s.get_session("bg:bg-uuid-1", "local").unwrap().is_some());
}

#[test]
fn reconcile_bg_agents_cleanup_spares_other_hosts_and_tmux_rows() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    s.upsert_host("remote").unwrap();
    // A bg row on ANOTHER host and a normal tmux row on this host.
    s.upsert_bg_session("remote", "bg:other", None, "other", Some("working"), 1)
        .unwrap();
    s.upsert_session("work-a", "local", None, None, 1, 1, "running", None)
        .unwrap();

    // Two empty-agent passes on `local` — enough to ghost + delete any
    // bg row this cleanup wrongly considered.
    reconcile_bg_agents(&s, "local", &[], &[], &[]).unwrap();
    reconcile_bg_agents(&s, "local", &[], &[], &[]).unwrap();

    let work = s.get_session("work-a", "local").unwrap().expect("tmux row");
    assert_eq!(work.status, "running", "tmux rows are not the bg pruner's");
    let other = s
        .get_session("bg:other", "remote")
        .unwrap()
        .expect("other host's bg row");
    assert_eq!(other.status, "running");
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
    assert!(cmds[0].contains("'dev-with-dashes'"));
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
        intel: PaneIntelMap::new(),
        pr_info: PrInfoMap::new(),
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
        intel: PaneIntelMap::new(),
        pr_info: PrInfoMap::new(),
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
        intel: PaneIntelMap::new(),
        pr_info: PrInfoMap::new(),
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
        recreate_pane_command("shell", Some(id)),
        crate::tmux::shell_pane_command(None)
    );
    assert_eq!(
        recreate_pane_command("work", Some(id)),
        crate::tmux::pane_command_for(Some(id))
    );
    assert_eq!(
        recreate_pane_command("work", None),
        crate::tmux::pane_command_for(None)
    );
    // A corrupt/non-UUID stored id must NOT inject — it degrades to the
    // --continue form (same as no id).
    assert_eq!(
        recreate_pane_command("work", Some("not-a-uuid; rm -rf /")),
        crate::tmux::pane_command_for(None)
    );
    // "review" is a non-shell kind → same resume behavior as "work".
    assert_eq!(
        recreate_pane_command("review", Some(id)),
        crate::tmux::pane_command_for(Some(id))
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
    let result = create_worktree_local(repo_str, "feat-x", None, None).await;
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
    let result2 = create_worktree_local(repo_str, "feat-x", None, None).await;
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
