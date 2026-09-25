# GitHub fixtures (work graph M6.1)

GraphQL answers as `gh api graphql` returns them from `api.github.com`,
recorded and sanitised: owners, repositories, logins and node ids are
invented; the shapes (field names, nesting, `null`s, per-field `errors`
next to `data`) are GitHub's.

| File | What |
|---|---|
| `viewer.json` | `viewer { login }` |
| `owner.json` | `repositoryOwner(login:)` for the site's owner scope |
| `search_mine_p1.json`, `_p2.json` | the `mine` search in two pages: an open issue with a linked branch (in_progress, two assignees, `Acme/api` mixed case), one with a closing PR, a sub-issue, closed as completed / not planned / duplicate |
| `nodes_two.json` | `nodes(ids:)` with a missing id: `null` plus a `NOT_FOUND` error |
| `repo_moved.json` | `repository(acme/legacy).issue(5)` answered by the transferred issue `acme/api#51` |
| `search_forbidden.json` | a search the token may not run: `FORBIDDEN` |
| `golden_list.json` | the normalised listing (`REGEN_TRACKER_GOLDENS=1`) |
