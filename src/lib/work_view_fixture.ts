// Test fixture (imported only by tests): Work view wire rows exactly as the
// spec's JSON shows them (`docs/superpowers/specs/2026-09-27-work-view-design.md`),
// overridable per test.
import type { WorkTask, WorkTaskLink } from './work_view';

export function link(over: Partial<WorkTaskLink> = {}): WorkTaskLink {
  return {
    link_id: 42,
    link_version: 3,
    state: 'active',
    primary: true,
    session_id: 7,
    name: 'ABC-12 login',
    host: 'mefistos',
    source: 'manual',
    strength: 'explicit',
    rule: null,
    why: 'branch abc-12-login · R3',
    created_at: 1790000000,
    decided_at: 1790000100,
    ended_at: null,
    end_reason: null,
    claude_status: 'idle',
    needs_you: false,
    archived: false,
    resumable: true,
    branch: 'abc-12-login',
    pr_url: null,
    cross_org: false,
    other_tasks: 1,
    ...over,
  };
}

export function task(over: Partial<WorkTask> = {}): WorkTask {
  return {
    task_id: 'item:12',
    item_id: 12,
    key: 'ABC-12',
    title: 'Login fails',
    url: 'https://acme.atlassian.net/browse/ABC-12',
    kind: 'tracker',
    tracker_id: 1,
    tracker_name: 'Jira (acme)',
    provider: 'jira',
    tracker_state: 'ok',
    status_category: 'in_progress',
    status_name: 'In Review',
    resolution: null,
    unavailable: false,
    unavailable_reason: null,
    assignees: ['Ana'],
    mine: true,
    org_id: 1,
    org_source: 'tracker',
    org_fenced: true,
    org_mixed: false,
    group: { id: 'tracker:1:ABC', label: 'ABC', source: 'tracker', rule_id: null, tracker_value: 'ABC' },
    counts: { active: 1, ended: 2, suggested: 1 },
    needs_you: false,
    review: false,
    last_activity_at: 1790000200,
    repos: ['acme/api'],
    placement_version: 0,
    sessions: [link()],
    sessions_more: 0,
    ...over,
  };
}
