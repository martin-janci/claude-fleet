# Jira Cloud fixtures (work graph M3.2)

Sanitised, hand-authored from Atlassian's documented response shapes for
REST API v3 (`/myself`, `/_edge/tenant_info`, `/project/search`, `/field`,
`/filter/favourite`, `POST /search/jql`, `POST /issue/bulkfetch`). No value
here came from a real site: account ids, cloud ids, names and emails are
invented, and the site is `https://acme.atlassian.net`.

What they cover:

| File | Case |
|---|---|
| `myself.json` | accountId, displayName, timeZone |
| `tenant_info.json` | cloudId → `instance_id` |
| `project_search_p1.json`, `project_search_p2.json` | key prefixes over two pages; `ABC` company-managed (classic), `TEAM` team-managed (next-gen) |
| `fields.json` | the sprint field found by `schema.custom = gh-sprint`, not by name |
| `sprint_projects.json` | only `ABC` has an open sprint (sprints are per project) |
| `filter_favourite.json` | two favourite filters, one whose JQL ends in `ORDER BY` |
| `search_mine_p1.json`, `search_mine_p2.json` | `nextPageToken` paging; an epic, a story with an ADF description in the active sprint, Done, Won't Do and Duplicate resolutions, a team-managed project-scoped status, an `undefined` status category |
| `search_repeat.json` | a page whose `nextPageToken` never changes (the loop guard) |
| `bulkfetch.json` | a by-id refresh: one found, one missing (`issueErrors`) |
| `bulkfetch_moved.json` | `OLD-5` asked for, answered as `NEW-5` (a moved issue) |
