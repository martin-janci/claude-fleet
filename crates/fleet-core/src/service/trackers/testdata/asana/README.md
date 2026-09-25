# Asana fixtures (work graph M6.2)

API answers from `https://app.asana.com/api/1.0`, recorded and sanitised:
gids, names and the workspace are invented; the shapes (`data`,
`next_page.offset`, `memberships[].section`, `/batch` results, the events
API's `sync` / `has_more` and its 412 body) are Asana's.

| File | What |
|---|---|
| `users_me.json` | `GET /users/me` with one workspace |
| `users_me_two_workspaces.json` | … with two (the probe asks which) |
| `probe_my_projects.json` | the probe's look at the user's open tasks' projects |
| `sections_p1.json`, `sections_p2.json` | the projects' sections (inference input) |
| `search_premium.json` / `search_not_premium.json` | search answered / refused with 402 |
| `tasks_mine_p1.json`, `_p2.json` | `mine` in two pages: a task in TWO projects (in progress by its first section), a backlog task, a subtask, a completed task, a milestone in a "Shipped" section |
| `batch_two.json` | `POST /batch` for three gids, the last a 404 |
| `events_p1.json` | `GET /events?resource=<project>&sync=` with one changed task (and a story, ignored) |
| `batch_changed.json` | the changed task, fetched |
| `events_expired.json` | the 412 body of an expired sync token, with a fresh one |
| `golden_list.json` | the normalised listing (`REGEN_TRACKER_GOLDENS=1`) |
