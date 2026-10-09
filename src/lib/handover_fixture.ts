// A hand-off a session wrote for TASK-223, as it came back between the markers.
export const TASK_223 = `TASK-223 handover: "Polish functionalities"
What the work is. Unknown. The only brief this session ever got was fleet handover #2589, which says "+New — Polish functionalities". That is a title with no scope.
What is done. No code, no commits, no file changes. The worktree is clean.
What the user decided. The user told this session to "just close ticket".
Where things are.

* Worktree: \`/Users/me/projects/claude-fleet/.worktrees/task-223-new\`
* Branch: \`task-223-new\`, at \`fe4f19c6\` (PR #472, v0.5.2). Nothing is committed on top.
* \`origin/main\` is at \`b52d5b28\` (PR #791), more than 300 PRs ahead.

What blocked closing it.

* \`work {action: task, task_id: "ref:TASK-223"}\` → \`E_NOTFOUND\`.
* \`whoami\` → \`E_NOTFOUND\`. Fleet had not reconciled the session.

Next steps.

1. Run \`whoami\` again. Once the session is linked, \`work_link {action: set_status, item_id, status: "done"}\`.
2. If it is still forbidden, ask the user to close the ticket in the Work view.
3. Once it is closed, the branch and its worktree can be removed.

Gotchas.

* Don't treat "Polish functionalities" as a spec.
* Don't use bare \`git stash\`. The stash stack is shared across worktrees.`;
