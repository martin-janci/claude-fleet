use super::*;

fn args(
    worktree_id: Option<i64>,
    new_worktree: Option<&str>,
    kind: Option<&str>,
) -> NewSessionArgs {
    NewSessionArgs {
        host_alias: "local".into(),
        project_id: 1,
        worktree_id,
        name: String::new(),
        call_id: None,
        new_worktree: new_worktree.map(Into::into),
        base_branch: None,
        kind: kind.map(Into::into),
        start_command: None,
        friendly_name: None,
    }
}

fn seeded() -> (Store, i64, i64) {
    let s = Store::open_in_memory().expect("store");
    s.upsert_host("local").unwrap();
    s.upsert_host("mefistos").unwrap();
    let pid = s.upsert_project("o", "r", "/tmp/o/r").unwrap();
    assert_eq!(pid, 1);
    let main_id = s
        .upsert_worktree(pid, "main", "/tmp/o/r", Some("main"))
        .unwrap();
    let feat_id = s
        .upsert_worktree(pid, "feat-x", "/tmp/o/r/.worktrees/feat-x", Some("feat-x"))
        .unwrap();
    (s, main_id, feat_id)
}

#[test]
fn deterministic_name_when_free() {
    let (s, main_id, feat_id) = seeded();
    assert_eq!(
        fill_session_name(&s, &args(Some(main_id), None, None)).unwrap(),
        "dev-o-r"
    );
    assert_eq!(
        fill_session_name(&s, &args(Some(feat_id), None, None)).unwrap(),
        "dev-o-r--feat-x"
    );
    assert_eq!(
        fill_session_name(&s, &args(None, Some("blue-sirius"), None)).unwrap(),
        "dev-o-r--blue-sirius"
    );
    assert_eq!(
        fill_session_name(&s, &args(Some(main_id), None, Some("shell"))).unwrap(),
        "dev-o-r-term"
    );
}

#[test]
fn appends_generated_pair_when_deterministic_name_is_taken() {
    let (s, main_id, _) = seeded();
    s.upsert_session(
        "dev-o-r",
        "local",
        Some(1),
        Some(main_id),
        1,
        1,
        "running",
        None,
    )
    .unwrap();
    let name = fill_session_name(&s, &args(Some(main_id), None, None)).unwrap();
    let suffix = name.strip_prefix("dev-o-r--").expect("pair appended");
    assert!(
        crate::service::names::adjectives()
            .iter()
            .any(|a| suffix.starts_with(&format!("{a}-"))),
        "{name}"
    );
    // The same name on another host does not count as taken.
    s.upsert_session(
        "dev-o-r--feat-x",
        "mefistos",
        Some(1),
        None,
        1,
        1,
        "running",
        None,
    )
    .unwrap();
    assert_eq!(
        fill_session_name(&s, &args(None, Some("feat-x"), None)).unwrap(),
        "dev-o-r--feat-x"
    );
}

#[test]
fn taken_slugs_include_worktrees_tmux_suffixes_and_friendly_names() {
    let (s, main_id, _) = seeded();
    s.upsert_session(
        "dev-o-r--amber-vega",
        "local",
        Some(1),
        Some(main_id),
        1,
        1,
        "running",
        None,
    )
    .unwrap();
    s.set_friendly_name("local", "dev-o-r--amber-vega", Some("Blue Sirius"))
        .unwrap();
    let taken = project_taken_slugs(&s, 1, "o", "r").unwrap();
    assert!(taken.contains("feat-x"), "worktree name");
    assert!(taken.contains("main"), "worktree name");
    assert!(taken.contains("amber-vega"), "tmux suffix");
    assert!(taken.contains("blue-sirius"), "slugified friendly name");
}

#[test]
fn dots_and_colons_are_mapped_so_the_name_validates() {
    let (s, _, _) = seeded();
    let v12 = s
        .upsert_worktree(1, "v1.2", "/tmp/o/r/.worktrees/v1.2", Some("v1.2"))
        .unwrap();
    let name = fill_session_name(&s, &args(Some(v12), None, None)).unwrap();
    assert_eq!(name, "dev-o-r--v1-2");
    crate::validate::tmux_name(&name).expect("filled name validates");
}

#[tokio::test]
async fn new_session_with_empty_name_is_filled_and_validated_end_to_end() {
    // Drive the real service entry point. The project's base_path does not
    // exist, so the call fails at the worktree step (E_GIT_SETUP, from
    // bash) — AFTER the empty name was minted and passed
    // `validate::tmux_name`. Before this change the same call failed with
    // E_INVALID ("session name must not be empty"). No tmux, no network.
    let s = Store::open_in_memory().expect("store");
    s.upsert_host("local").unwrap();
    s.upsert_project("o", "r", "/nonexistent/claude-fleet-test/o/r")
        .unwrap();
    let store = Mutex::new(s);
    let ssh = Arc::new(SshClient::new());
    let reg = CancellationRegistry::new();

    let err = new_session(args(None, Some("blue-sirius"), None), &store, &ssh, &reg)
        .await
        .expect_err("repo is not on disk");
    assert_eq!(err.code, "E_GIT_SETUP", "{err:?}");

    // Whitespace-only is treated as empty too.
    let mut ws = args(None, Some("red-comet"), None);
    ws.name = "   ".into();
    let err = new_session(ws, &store, &ssh, &reg).await.unwrap_err();
    assert_eq!(err.code, "E_GIT_SETUP", "{err:?}");

    // Control: an explicit invalid name is still rejected up front.
    let mut bad = args(None, Some("red-comet"), None);
    bad.name = "bad.name".into();
    let err = new_session(bad, &store, &ssh, &reg).await.unwrap_err();
    assert_eq!(err.code, "E_INVALID", "{err:?}");
}

#[test]
fn generated_pair_avoids_slugs_already_used_on_the_project() {
    let (s, main_id, _) = seeded();
    s.upsert_session(
        "dev-o-r",
        "local",
        Some(1),
        Some(main_id),
        1,
        1,
        "running",
        None,
    )
    .unwrap();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..50 {
        let name = fill_session_name(&s, &args(Some(main_id), None, None)).unwrap();
        let suffix = name.strip_prefix("dev-o-r--").unwrap().to_string();
        // Worktree names on the project are off-limits.
        assert_ne!(suffix, "feat-x");
        seen.insert(suffix);
    }
    assert!(seen.len() > 1, "names are random, not fixed");
}
