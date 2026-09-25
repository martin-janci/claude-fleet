# Jira Data Center fixtures (work graph M6.5)

REST API v2 answers of a Data Center site at `https://jira.corp.example/jira`
(a context path), recorded and sanitised: ids, keys, names and the host are
invented; the shapes are Data Center's — `/search` paged by `startAt` /
`total`, `name` as the user id, a plain-text description, the sprint field
as legacy `Sprint@…[…,state=ACTIVE,name=…]` strings, the Epic Link as a
key, and `warningMessages` from `validateQuery: warn`.

| File | What |
|---|---|
| `myself.json` | `GET /rest/api/2/myself` |
| `projects.json` | `GET /rest/api/2/project` (one key in lower case) |
| `fields.json` | `GET /rest/api/2/field`: the sprint and Epic Link fields by `schema.custom`, and a decoy named "Sprint" |
| `sprint_projects.json` | the bounded probe search for open sprints |
| `search_mine_p1.json`, `_p2.json` | `mine` in two pages: an epic, a story in the active sprint linked to it, a sub-task, Fixed, Won't Do, and an unassigned To Do |
| `fetch_two.json` | a by-reference search for three, one key missing (a warning) |
| `fetch_moved.json` | `key in (OLD-7)` answered by the moved `PLAT-7` |
| `golden_list.json` | the normalised listing (`REGEN_TRACKER_GOLDENS=1`) |

A CAPTCHA lockout is a 403 with `X-Seraph-LoginReason:
AUTHENTICATION_DENIED` (scripted in the test). The self-signed CA case
generates its CA and a `localhost` certificate with `openssl` at test time:
no private key is committed.
