//! The worker guard's reading of a Bash command (orchestration §7.2).

use super::*;

#[test]
fn a_persons_steps_are_refused() {
    for cmd in [
        "gh pr merge 12 --squash",
        "gh pr ready",
        "cd /w && gh pr merge --auto",
        "git push origin main",
        "git push -u origin HEAD:main",
        "git push origin +feature:refs/heads/master",
        "git -C /w/repo push origin main",
        "FOO=1 git push --force origin main",
        "git push --all",
        "sudo /usr/bin/git push origin master",
        "jira issue move ABC-1 Done",
        "echo ok; acli jira workitem transition --key ABC-1",
        "true && linear issue update",
    ] {
        assert!(deny_reason(cmd, Some("feat/x")).is_some(), "{cmd}");
    }
}

#[test]
fn a_workers_own_steps_pass() {
    for cmd in [
        "git push -u origin feat/x",
        "git push origin HEAD",
        "git push",
        "gh pr create --draft --fill",
        "gh pr view 12",
        "git commit -m 'merge main into feat'",
        "echo 'gh pr merge' > notes.txt",
        "cargo test -- push",
        "grep -r jira src",
        "git log main..HEAD",
    ] {
        assert!(deny_reason(cmd, Some("feat/x")).is_none(), "{cmd}");
    }
}

#[test]
fn a_bare_push_from_the_default_branch_is_refused() {
    assert!(deny_reason("git push", Some("main")).is_some());
    assert!(deny_reason("git push origin", Some("master")).is_some());
    assert!(deny_reason("git push origin HEAD", Some("main")).is_some());
    assert!(
        deny_reason("git push", None).is_none(),
        "an unknown branch is not guessed"
    );
}
