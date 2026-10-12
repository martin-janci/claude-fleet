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
fn the_script_quotes_every_value() {
    let s = merge_tree_script(&["/w/it's"], &["a;b".into(), "c".into()]);
    assert!(s.contains(&shq("/w/it's")));
    assert!(s.contains(&shq("a;b")));
    assert!(s.contains("__FLEET_MT 0 1"));
    assert!(!s.contains("__FLEET_MT 1 0"));
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
}
