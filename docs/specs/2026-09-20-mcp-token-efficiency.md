# MCP token efficiency — measured audit and the changes it produced

**Date:** 2026-09-20 · **Measured against:** a live fleet-hub at `v0.2.23`
(schema 37), 5 hosts, 55 session rows, 249 worktrees. Every number below comes
from the server itself (`tools/list` and read-only `tools/call` payloads),
not from an estimate. Token figures use ~3.7 chars/token for JSON. Sections
1–4 are the audit as it stood; § 5 is what landed and what it measures now.

## 1. What the surface cost (before)

| Item | Chars | ~Tokens |
| --- | ---: | ---: |
| 73 tool definitions (`tools/list`) | 64,265 | **17,370** |
| — of which descriptions | 24,668 | 6,670 |
| — of which input schemas | 38,660 | 10,450 |
| Median tool definition | 753 | 205 |

Anthropic's own threshold for switching to on-demand tool loading is "10+
tools or >10k tokens of definitions" ([tool search
tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool));
tool-selection accuracy is documented to degrade past 30–50 tools. We are at
73 tools / 17.4k tokens — well over both.

Claude Code already defers this surface (its MCP tool search loads definitions
on demand), so **for a Claude Code client the start-up cost is largely
mitigated today**. It is not mitigated for: the Messages API MCP connector
without `default_config.defer_loading`, paired phone/browser clients, smaller
models, and any client that lists tools eagerly. And deferral changes *where*
the budget goes rather than removing it: discovery now happens by regex/BM25
over **tool names, descriptions, argument names and argument descriptions**,
which makes description *wording* a functional concern, not only a size one.

## 2. Where the tokens actually go: results, not definitions

Measured, single read-only calls on this fleet:

| Call | Result chars | ~Tokens |
| --- | ---: | ---: |
| `list_worktrees {}` (249 rows) | 76,352 | **20,640** |
| `list_projects { summary: false }` | 65,646 | 17,740 |
| `list_sessions { summary: false }` (55 rows) | 40,576 | 10,970 |
| `list_sessions {}` (summary, 55 rows) | 9,650 | 2,610 |
| `list_projects {}` (summary) | 6,489 | 1,750 |
| `usage_report {}` | 5,941 | 1,610 |
| `list_accounts {}` | 2,189 | 590 |
| `list_hosts {}` | 1,584 | 430 |
| `fleet_health {}` | 1,552 | 420 |
| `session_history { session_id }` | 367 | 100 |
| `whoami`, `peer_status`, `capture_session` (idle pane) | < 200 | < 50 |

**One careless `list_worktrees` costs more than the entire tool surface.** On
a long-running controller session the recurring result budget dominates the
one-off definition budget by an order of magnitude, so result shaping is the
higher-leverage half of this work.

## 3. Findings

### F1 — `list_worktrees` has no summary, no limit, no cap (highest impact)
Its only parameter is `project_id`. It returns the full worktree record plus
an `occupants` array for every worktree on every host, pretty-printed with
nulls: 20.6k tokens here and growing linearly with the fleet.
*Fix:* `summary` (default true → `id, project_id, host_alias, name, branch,
occupied`), `limit`, `host_alias`, and `ok_json_compact`. Estimated 20.6k →
~1.4k tokens.

### F2 — pretty-printed, null-carrying results on the remaining list tools
`ok_json` (67 call sites) uses `to_string_pretty` and keeps `null` fields;
`ok_json_compact` (11 sites) strips both. Measured overhead on the tools still
on `ok_json`: `list_worktrees` 24%, `fleet_health` 28%, `list_hosts` 25%,
`usage_report` 25%, `list_accounts` ~25%. `list_sessions` / `list_projects`
are already compact — the pattern exists, it is just not applied everywhere.
*Fix:* make `ok_json_compact` the default for every list/report-shaped tool.

### F3 — schema noise is ~12% of the definition budget
Across the 73 schemas: `"$schema"` 64×, `"title"` 64×, `"format"` 69×
(`int64`/`uint` — meaningless to the model), `"default": null` 108×, plus
hard-wrapped multi-line field descriptions whose newlines and indentation are
pure padding. Mechanically stripping those and collapsing whitespace:
64,265 → 56,273 chars (**−12%, ~2.2k tokens**), with zero semantic loss.
*Fix:* a `slim_schema()` pass in `list_tools` / `get_tool` (post-process the
schemars output) plus single-line `///` docs in `params.rs`.

### F4 — descriptions carry workflow prose that belongs in the skill
Largest: `move_session` 1,533 chars, `session_conversation` 1,339,
`repair_session` 1,151, `dispatch_task` 706, `kill_session` 681. A description
should carry what the model needs to *choose* and *call* the tool (purpose,
the arguments that change behaviour, the error codes it can answer with).
Narrative — ladders, when-to-prefer-what, recovery procedure — belongs in
`skills/claude-fleet-control/SKILL.md`, which is loaded only when relevant,
whereas a description is paid for on every eager list and every search hit.
*Fix:* cap descriptions at ~400 chars, push the rest into the skill. Estimated
~1.5–2k tokens.

### F5 — `list_tools` is not scoped to the caller
`FleetTools::list_tools` returns `self.tool_router.list_all()` for every
caller. A `readonly` token is served ~30 tools it will be refused at call time
(`enforce_mode`), and a per-host session token is served the fleet-admin
tools. This is both a token cost and a selection-accuracy cost: the model
tries a tool it cannot use.
*Fix:* filter the served list by `Caller` (`TokenMode`, host scope) — the same
predicate `enforce_mode` / `enforce_admin` already implement. A core
session-driving profile of 26 tools measures 8.4k tokens (7.5k with F3
applied) against 17.4k for the full surface — **−57%** for the common caller,
and it needs no new concept, only reuse of the existing gate.

### F6 — deprecated tools are still served
`peek_session` is documented as "Deprecated: use `session_transcript`" and
still occupies a definition and a search slot. Deprecated-but-served is the
worst of both: it costs tokens and it competes for selection.
*Fix:* remove it in the next release that already churns tool definitions.

### F7 — no `annotations`, no `outputSchema`
Neither is a token win by itself (both *add* bytes to definitions), so this is
a deliberate non-finding with one exception: `readOnlyHint` /
`destructiveHint` on the ~20 mutating tools lets clients auto-approve reads
and gate writes, which removes confirmation round trips. Worth ~15 chars per
tool. `outputSchema` is not worth it here — errors already travel as
`structuredContent`, which is the part that actually saves retries.

## 4. Principles (what "natural and functional for AI" means here)

1. **Cheapest call that answers the question.** Every tool gets a slim default
   and an explicit widening flag — never the reverse. A default that is safe
   for a 3-row fleet and ruinous at 250 rows is a bug.
2. **Filter at the server, not in the model's head.** Every list tool takes
   the filters its callers actually use (`host_alias`, `project_id`, `state`,
   `tag`) plus `limit`. Paging beats truncation: a capped result with no way
   to ask for the rest forces a full re-read.
3. **One round trip beats three.** `run_prompt` (send + wait + read) exists
   because the three-call form costs three results and three turns. Bounded
   long-polls (`wait_for_session`, `wait_for_task`) exist so nobody writes a
   poll loop over `list_sessions` — the single most expensive anti-pattern on
   this API.
4. **Definitions are for selection; skills are for workflow.** Keywords a user
   would actually say ("stuck", "OOM", "cost", "transcript") belong in the
   first line of the description, because tool search matches on it. Procedure
   belongs in the skill.
5. **Errors are structured and final.** `E_*` + `structuredContent` lets the
   model correct or stop instead of retrying blind. Already true — keep it.
6. **Scope the surface to the caller.** The tools a token may not call should
   not be in its context.

## 5. What shipped

All eight items landed together (definition churn is batched, per the
MCP-prefix-stability rule in `skills/claude-fleet-repo/SKILL.md`).

| # | Change | Where |
| --- | --- | --- |
| 1 | `list_worktrees` → `{total, worktrees}`, slim rows, `limit` (default 100, `0` = no cap), `host_alias` filter | `mcp/tools/repo.rs`, `support.rs` |
| 2 | `tools/list` scoped to the caller by the same predicates that gate the call | `mcp/tools/present.rs`, `mod.rs` |
| 3 | `ok_json` is compact; the list/report tools drop nulls via `ok_json_compact` | `mcp/tools/*.rs` |
| 4 | `slim_schema` strips `$schema`, `title`, numeric `format`s, `"default": null`, and collapses wrapped doc comments | `mcp/tools/present.rs` |
| 5 | Description diet on `move_session`, `repair_session`, `session_conversation` (−55% each) | `lifecycle.rs`, `orchestration.rs` |
| 6 | `peek_session` removed (router, policy row, params, docs) | `session_ops.rs`, `guard.rs` |
| 7 | `list_projects` takes `limit` | `mcp/tools/repo.rs` |
| 8 | `readOnlyHint` / `destructiveHint` from the policy row | `mcp/tools/present.rs` |

### Measured after

Definitions (name + description + schema, the way a model pays for them):

| Caller | Tools | Bytes | ~Tokens | vs before |
| --- | ---: | ---: | ---: | ---: |
| master | 72 | 54,695 | 14,780 | −15% |
| per-host `full` (a provisioned session) | 62 | 47,734 | 12,900 | −26% |
| `readonly` | 36 | 20,623 | 5,570 | −68% |

Results, on the same 249-worktree fleet:

| Call | Before | After |
| --- | ---: | ---: |
| `list_worktrees {}` | ~20,640 tok | ~3,250 tok (100 of 249 rows, `total` carried) |
| `fleet_health`, `list_hosts`, `usage_report`, `list_accounts` | — | −25% each (compact, nulls dropped) |

`mcp::tools::tests::the_served_definition_budget_stays_bounded` holds the
surface to 56,000 bytes and asserts a `readonly` token is served under half
of it; `the_served_tool_list_matches_the_call_gates` walks every router tool
against every caller shape, so the list and the gate cannot drift apart.

### The one cross-mode catch

`list_worktrees` is not only an agent tool: the desktop paired to a hub calls
it through the same MCP surface (`src-tauri/src/backend/remote.rs`) and draws
the whole project tree from it. Defaults shaped for an agent — slim rows, one
page, an envelope — would have silently truncated that tree. The desktop now
asks for `summary: false, limit: 0` and unwraps `{total, worktrees}`; `limit:
0` means "no cap", the same convention `capture_session.max_lines` already
uses. Null-stripping survives the round trip because a missing field
deserializes back to `None`, which
`null_stripped_results_still_deserialize` pins down.

### Deliberately not done

- **`outputSchema`.** It *adds* definition bytes, and the part that actually
  saves retries — a structured error with an `E_*` code — is already there as
  `structuredContent`.
- **A description cap for its own sake.** The remaining 400–700 char
  descriptions (`send_prompt`, `kill_session`, `dispatch_task`,
  `list_sessions`) carry the call contract: parameters, return shape, error
  codes. Cutting them would trade tokens for wrong calls, which cost more.
- **Renaming tools into prefixed families.** It would help tool search, but
  every rename invalidates connected clients' cached definitions and breaks
  callers; not worth it for the existing `list_*` / `repo_*` / `session_*`
  families, which are already consistent enough to match one search.

## 6. Client-side follow-up (no server change)

- The `claude-fleet-control` skill carries the cost table and the cheap-first
  rules (§ *Token discipline*), so a controller session spends its budget on
  work rather than on fleet dumps.
- For the Messages API MCP connector, set `defer_loading` on the
  `mcp_toolset` entry's `default_config` and keep the 3–5 hot tools
  (`list_sessions`, `send_prompt`, `run_prompt`, `session_transcript`)
  non-deferred.

## Sources

- [Tool search tool — Claude Platform Docs](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool)
- [Advanced tool use — Anthropic Engineering](https://www.anthropic.com/engineering/advanced-tool-use)
- [Effective context engineering for AI agents — Anthropic Engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
