# Task result, completion gate and follow-ups: mission steps

Import with "Import plan" on the mission. Detail of each step:
`docs/superpowers/plans/2026-10-13-task-result-handover-execution.md` (who, which files) and
`docs/superpowers/plans/2026-10-13-task-result-handover.md` (tests, code).

| # | Step | Needs | Lane | Status | Repo |
|---|---|---|---|---|---|
| 0.1 | Scaffold modules, migration 199 and WorkItemRow.origin_task_id | | Orchestrator | todo | claude-fleet |
| 1.1 | Report carries done, decisions, remaining, context and latest_item_result | 0.1 | Backend | todo | claude-fleet |
| 1.2 | Wrap-up request service with role wrapup | 0.1 | Backend | todo | claude-fleet |
| 1.3 | Completion gate and agent refusal of done past it | 0.1 | Backend | todo | claude-fleet |
| 1.4 | Follow-up store with create_followup and followups_of | 0.1 | Backend | todo | claude-fleet |
| 1.5 | TS report mirror, ReportCard sections, task_result.ts and TaskResultPanel | | Frontend | todo | claude-fleet |
| 1.6 | Merge wave A, push and Buildkite build 1 | 1.1, 1.2, 1.3, 1.4, 1.5 | Orchestrator | todo | claude-fleet |
| 2.1 | Result view joining report, evidence, gate, PR and follow-ups | 1.6 | Backend | todo | claude-fleet |
| 2.2 | Follow-up service with scope fence and index or title | 1.6 | Backend | todo | claude-fleet |
| 2.3 | OriginFacts rendered first in the start brief | 1.6 | Backend | todo | claude-fleet |
| 2.4 | Mount the panel in TicketCard, task page and conversation | 1.6 | Frontend | todo | claude-fleet |
| 2.5 | Merge wave B, push and Buildkite build 2 | 2.1, 2.2, 2.3, 2.4 | Orchestrator | todo | claude-fleet |
| 3.1 | Wire MCP actions and desktop commands, hand-edit generated files | 2.5 | Backend | todo | claude-fleet |
| 3.2 | Docs in work-graph.md and status.md | 2.5 | Docs | todo | claude-fleet |
| 3.3 | Merge wave C, push and Buildkite build 3 | 3.1, 3.2 | Orchestrator | todo | claude-fleet |
| 4.1 | Independent review of the branch against the spec | 3.3 | Review | todo | claude-fleet |
| 4.2 | Merge origin/main and renumber the migration if needed | 4.1 | Orchestrator | todo | claude-fleet |
| 4.3 | Push and Buildkite build 4 on the final HEAD | 4.2 | Orchestrator | todo | claude-fleet |
| 4.4 | Open one PR to main and list what was not verified | 4.3 | Orchestrator | todo | claude-fleet |
