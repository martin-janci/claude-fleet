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
        // A flag's value or a summary is never the verb.
        "jira issue create -s \"Show login error\"",
        "jira issue create \"List view broken\"",
        "linear issue create --title \"List view broken\"",
        "asana task create --name \"Get logs\"",
        "acli jira workitem create --summary \"view is blank\"",
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

/// `gh`'s global options, odd spacing, `gh api` and a shell wrapper do not
/// hide a merge or a ready.
#[test]
fn a_merge_or_ready_in_another_form_is_refused() {
    for cmd in [
        "gh -R acme/app pr merge 41 --squash --admin",
        "gh --repo acme/app pr ready 41",
        "gh --repo=acme/app pr merge",
        "gh pr -R acme/app merge 41",
        "gh  pr merge 41",
        "gh --hostname ghe.example.com pr merge 3",
        "gh api -X PUT repos/acme/app/pulls/41/merge",
        "gh api -XPUT /repos/acme/app/pulls/41/merge",
        "gh api --method=put repos/acme/app/pulls/41/merge",
        "gh api repos/acme/app/pulls/41/merge -f merge_method=squash",
        "gh api graphql -f query='mutation { mergePullRequest(input: {pullRequestId: \"x\"}) { clientMutationId } }'",
        "gh api graphql -f query='\nmutation {\n  markPullRequestReadyForReview(input: {pullRequestId: \"x\"}) { clientMutationId }\n}'",
        "gh api graphql -F query='mutation { enablePullRequestAutoMerge(input: {}) { clientMutationId } }'",
        "bash -c \"gh pr merge 1\"",
        "sh -lc 'cd /w && git push origin main'",
        "eval gh pr merge 3",
        "echo 41 | xargs -I{} gh pr merge {}",
        "timeout 30 gh pr merge 1",
        "timeout -s KILL 30 gh pr ready 1",
    ] {
        assert!(deny_reason(cmd, Some("feat/x")).is_some(), "{cmd}");
    }
}

#[test]
fn reading_a_pr_through_another_form_passes() {
    for cmd in [
        "gh -R acme/app pr view 41",
        "gh --repo acme/app pr checks 41",
        "gh api repos/acme/app/pulls/41/merge",
        "gh api repos/acme/app/pulls/41",
        "gh api -X PATCH repos/acme/app/issues/41 -f title=x",
        "gh api graphql -f query='query { viewer { login } }'",
        "bash -c \"cargo test\"",
        "xargs -n 1 echo",
        "timeout 30 cargo test",
    ] {
        assert!(deny_reason(cmd, Some("feat/x")).is_none(), "{cmd}");
    }
}

/// A command that changes folder or branch before a bare push leaves the
/// pushed branch unknown: the branch fleet recorded is not the one pushed.
#[test]
fn a_bare_push_after_changing_folder_or_branch_is_refused() {
    for cmd in [
        "cd ~/src/api && git merge feat/x && git push",
        "git -C ~/src/api push",
        "git -C ~/src/api push origin",
        "git checkout main && git push origin HEAD",
        "git switch main; git push",
        "pushd /base && git push",
    ] {
        assert!(deny_reason(cmd, Some("feat/x")).is_some(), "{cmd}");
    }
    for cmd in [
        "cd /w/feat && git push -u origin feat/x",
        "git checkout feat/y && git push origin feat/y",
        "git -C /w/feat push origin HEAD:feat/x",
        "cd /w && cargo test",
    ] {
        assert!(deny_reason(cmd, Some("feat/x")).is_none(), "{cmd}");
    }
}

/// An MCP tool: a tracker's or GitHub's write, and a PR's merge or ready on
/// any server, are a person's; a read, fleet's own tools and other servers'
/// tools are the worker's.
#[test]
fn an_mcp_write_to_a_tracker_or_a_pr_is_refused() {
    for tool in [
        "mcp__atlassian__transitionJiraIssue",
        "mcp__atlassian__addCommentToJiraIssue",
        "mcp__atlassian__editJiraIssue",
        "mcp__Atlassian__createConfluencePage",
        "mcp__company-jira__frobnicate",
        "mcp__linear__create_issue",
        "mcp__asana__update_tasks",
        "mcp__github__merge_pull_request",
        "mcp__github__update_pull_request",
        "mcp__github__enable_pr_auto_merge",
        "mcp__github__push_files",
        "mcp__gh-enterprise__mergePullRequest",
        "mcp__forge__mark_pr_ready",
    ] {
        assert!(mcp_deny_reason(tool).is_some(), "{tool}");
    }
    for tool in [
        "mcp__atlassian__getJiraIssue",
        "mcp__atlassian__searchJiraIssuesUsingJql",
        "mcp__atlassian__atlassianUserInfo",
        "mcp__atlassian__lookupJiraAccountId",
        "mcp__github__pull_request_read",
        "mcp__github__list_issues",
        "mcp__github__get_me",
        "mcp__claude-fleet__work_link",
        "mcp__playwright__browser_click",
        "mcp__git__git_merge",
        "Bash",
        "Read",
    ] {
        assert!(mcp_deny_reason(tool).is_none(), "{tool}");
    }
}
