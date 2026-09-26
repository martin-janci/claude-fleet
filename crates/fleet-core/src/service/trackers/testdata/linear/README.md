# Linear fixtures (work graph M6.4)

GraphQL answers from `https://api.linear.app/graphql`, recorded and
sanitised: ids, names and the workspace are invented; the shapes (`issues`
connections, `previousIdentifiers`, `state.type`, a field error with a
`path` that nulls the non-null root, `extensions.code`) are Linear's.

| File | What |
|---|---|
| `probe.json` | viewer, organization, teams: `ENG` with an active cycle, `ops` without cycles |
| `probe_no_cycles.json` | a workspace whose only team has no cycles (no sprint view) |
| `issues_mine_p1.json`, `_p2.json` | two pages: started (in the active cycle), unstarted, a backlog sub-issue, completed, canceled, and a triage issue moved from team OPS (`previousIdentifiers`) |
| `fetch_two.json` | three `issue(id:)` lookups, the last missing: `Query.issue` is non-null, so `data` is `null` and one `INVALID_INPUT` error names the alias in its `path` |
| `fetch_two_retry.json` | the two present issues, asked for again without the missing one |
| `fetch_moved.json` | `issue(id: "OPS-3")` answered by the moved `ENG-110` |
| `ratelimited_complexity.json` | a complexity-limit error (`RATELIMITED`) |
| `unauthenticated.json` | a refused key (`AUTHENTICATION_ERROR`) |
| `forbidden.json` | a view the key may not read (`FORBIDDEN`) |
| `golden_list.json` | the normalised listing (`REGEN_TRACKER_GOLDENS=1`) |
