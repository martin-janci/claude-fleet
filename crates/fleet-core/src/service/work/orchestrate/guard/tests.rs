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
        "jira issue comment add PAY-1 done",
        "jira issue edit PAY-1 -s Done",
        "jira issue create -t Bug",
        "jira",
        "acli jira workitem edit --key PAY-1 --summary x",
        "jira issue view PAY-1; jira issue assign PAY-1 me",
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
        "jira issue view PAY-123 --comments 5",
        "jira issue list -s Done --plain",
        "jira me",
        "acli jira workitem view PAY-1",
        "acli jira workitem search --jql project=PAY",
        "linear issue list",
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
