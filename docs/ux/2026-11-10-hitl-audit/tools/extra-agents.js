// Agent actions waiting for a person (agents.ts AgentsStatus / AgentAction /
// ActionAbout), for the Details capture. Loaded after extra-inbox.js.
(() => {
  const NOW = Math.floor(Date.now() / 1000);
  // Invented text in the shape agents write: markdown bold, paragraphs, quotes.
  const description =
    '**Context**\n\nThe report page of the demo app, "Custom" date range. Seen in DEMO-17, confirmed by capturing the request.\n\n' +
    '**Current behaviour**\n\nThe date picker sends times with a fixed +02:00 offset, even after the switch to +01:00, so a chosen 10:00–12:00 is computed as 09:00–11:00.\n\n' +
    'The preset ranges (Today / Yesterday / This month) are not affected.\n\n**Expected behaviour**\n\nA custom range is sent in the offset valid on the chosen date.';
  const args = { issue_key: 'DEMO-42', description, format: 'markdown', returnIssue: true };
  const action = (id, key) => ({
    id, job_id: 7, kind: 'mcp_call', state: 'pending', decided_by: null, decided_at: null, executed_at: null, error: null,
    created_at: NOW - 6 * 86400,
    payload: JSON.stringify({ action: 'update', store: 'tracker', binding: 'jira', tool: 'jira_update_issue', arguments: { ...args, issue_key: key },
      body: `jira.jira_update_issue(${JSON.stringify({ ...args, issue_key: key }, null, 2)})` }),
  });
  const about = (id, key) => ({ id, rule_id: 4, from: null, subject: 'a scheduled run', title: `Jira: update issue ${key} (description, format, returnIssue)`, what: 'Jira: update issue', group: `${id}:solo` });
  const status = { rules: [], jobs: [], pending: [action(1, 'DEMO-42')], pending_total: 1, pending_about: [about(1, 'DEMO-42')], recent: [] };
  window.__FLEET_EXTRA__ = { ...(window.__FLEET_EXTRA__ || {}), agents_status: () => status };
})();
