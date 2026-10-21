//! The integration check's script and its reading (orchestration O7).

use super::*;

#[test]
fn a_conflict_names_its_paths_and_old_git_says_nothing() {
    let out = "__FLEET_MT 0 1 1\n__FLEET_MTF 0 1 src/a.rs\n__FLEET_MTF 0 1 src/b.rs\n\
               __FLEET_MT 0 2 0\n__FLEET_MT 1 2 128\n__FLEET_MT 2 9 0\n";
    let got = parse_merge_tree(out, 3);
    assert_eq!(
        got,
        vec![
            (
                0,
                1,
                "conflict".into(),
                vec!["src/a.rs".into(), "src/b.rs".into()]
            ),
            (0, 2, "clean".into(), vec![]),
            (1, 2, "unknown".into(), vec![]),
        ]
    );
    assert!(parse_merge_tree("__FLEET_MT_OLD\n", 3).is_empty());
    assert!(parse_merge_tree("__FLEET_MT_NOREPO\n", 3).is_empty());
}

#[test]
fn a_contained_head_is_left_out_and_of_two_equal_heads_the_later() {
    // 0 is in 2 (a merge of it); 1 and 3 are the same head.
    let out = "__FLEET_MTA 0 2\n__FLEET_MTA 1 3\n__FLEET_MTA 3 1\n__FLEET_MTA 2 9\n";
    let got: Vec<usize> = parse_contained(out, 4).into_iter().collect();
    assert_eq!(got, vec![0, 3]);
    assert!(parse_contained("", 4).is_empty());
}

#[test]
fn a_title_names_every_branch() {
    assert_eq!(joined(&["A"]), "A");
    assert_eq!(joined(&["A", "B"]), "A and B");
    assert_eq!(joined(&["A", "B", "C"]), "A, B and C");
}

fn branch(item_id: i64, head: &str) -> Branch {
    Branch {
        item_id,
        label: format!("TASK-{item_id}"),
        head: head.into(),
        host: "h".into(),
        path: "/w".into(),
        project_id: 1,
    }
}

/// Mission #6 (2026-10-21): one set of heads raises ONE integration that
/// waits for every live branch, however many pairs conflict, and the same
/// set never raises it twice.
#[test]
fn one_integration_covers_every_conflicting_pair() {
    let s = Store::open_in_memory().unwrap();
    let m = s
        .create_mission(
            &crate::store::NewMission {
                name: "m",
                goal: "g",
                ..Default::default()
            },
            "person:1",
        )
        .unwrap();
    let (a, b, c) = (
        branch(300, "aaaa"),
        branch(301, "bbbb"),
        branch(306, "cccc"),
    );
    let live = [&a, &b, &c];
    let conflicts = vec![
        (&a, &b, vec!["CHANGELOG.md".to_string()]),
        (&b, &c, vec!["src/toolbar.tsx".to_string()]),
    ];
    integration_card(&s, &m, &live, &conflicts);
    integration_card(&s, &m, &live, &conflicts);
    let cards = s.mission_cards(m.id, 50).unwrap();
    assert_eq!(cards.len(), 1, "{cards:?}");
    let tree = &cards[0].payload.as_ref().unwrap()["tree"][0];
    assert_eq!(
        tree["title"],
        "Integrate TASK-300, TASK-301 and TASK-306 into one branch"
    );
    let notes = tree["notes"].as_str().unwrap();
    assert!(
        notes.contains("TASK-300 × TASK-301: CHANGELOG.md"),
        "{notes}"
    );
    assert!(
        notes.contains("TASK-301 × TASK-306: src/toolbar.tsx"),
        "{notes}"
    );
    assert_eq!(tree["depends_on"].as_array().unwrap().len(), 3);
}

#[test]
fn the_script_quotes_every_value() {
    let s = merge_tree_script(&["/w/it's"], &["a;b".into(), "c".into()]);
    assert!(s.contains(&shq("/w/it's")));
    assert!(s.contains(&shq("a;b")));
    assert!(s.contains("__FLEET_MT 0 1"));
    assert!(!s.contains("__FLEET_MT 1 0"));
    assert!(s.contains("merge-base --is-ancestor"));
}

#[cfg(unix)]
fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = crate::proc::std_command("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Against a real repo: two branches that edit one line conflict, a third
/// that edits another file does not.
// The script runs on a fleet host (a Unix shell); a Windows runner's
// `bash` is not one.
#[cfg(unix)]
#[test]
fn real_git_tells_a_conflict_from_a_clean_merge() {
    let probe = crate::proc::std_command("git")
        .args(["merge-tree", "-h"])
        .output();
    let Ok(probe) = probe else { return };
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&probe.stdout),
        String::from_utf8_lossy(&probe.stderr)
    );
    if !help.contains("--write-tree") {
        return; // git older than 2.38
    }
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    git(p, &["init", "-q", "-b", "main"]);
    std::fs::write(p.join("a.txt"), "one\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "base"]);
    let mut heads = Vec::new();
    for (name, file, text) in [
        ("x", "a.txt", "two\n"),
        ("y", "a.txt", "three\n"),
        ("z", "b.txt", "other\n"),
    ] {
        git(p, &["checkout", "-qb", name, "main"]);
        std::fs::write(p.join(file), text).unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-qm", name]);
        heads.push(git(p, &["rev-parse", "HEAD"]));
    }
    // The first worktree is gone: the next one still answers.
    let script = merge_tree_script(&["/nonexistent/fleet-wt", p.to_str().unwrap()], &heads);
    let out = crate::proc::std_command("bash")
        .args(["-c", &script])
        .output()
        .unwrap();
    let got = parse_merge_tree(&String::from_utf8_lossy(&out.stdout), 3);
    assert_eq!(got[0], (0, 1, "conflict".into(), vec!["a.txt".into()]));
    assert_eq!(got[1].2, "clean");
    assert_eq!(got[2].2, "clean");
    assert!(parse_contained(&String::from_utf8_lossy(&out.stdout), 3).is_empty());

    // An integration that merged x and y contains both: they drop out, and
    // what is left (the merge and z) merges cleanly — nothing to card.
    git(p, &["checkout", "-q", "x"]);
    git(p, &["checkout", "-qb", "xy"]);
    let _ = git(p, &["merge", "-q", "--no-edit", "y"]);
    std::fs::write(p.join("a.txt"), "two and three\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "resolve"]);
    heads.push(git(p, &["rev-parse", "HEAD"]));
    let script = merge_tree_script(&[p.to_str().unwrap()], &heads);
    let out = crate::proc::std_command("bash")
        .args(["-c", &script])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let contained: Vec<usize> = parse_contained(&stdout, 4).into_iter().collect();
    assert_eq!(contained, vec![0, 1]);
    let live_conflicts: Vec<_> = parse_merge_tree(&stdout, 4)
        .into_iter()
        .filter(|(i, j, st, _)| {
            st == "conflict" && !contained.contains(i) && !contained.contains(j)
        })
        .collect();
    assert!(live_conflicts.is_empty(), "{live_conflicts:?}");
}
