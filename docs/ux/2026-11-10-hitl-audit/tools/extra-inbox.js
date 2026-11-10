// d1a extra answers for the fake Tauri IPC. Shapes from src/lib/*.ts (cited per command).
(() => {
  const NOW = Math.floor(Date.now() / 1000);
  const iso = (secsAgo) => new Date((NOW - secsAgo) * 1000).toISOString();
  const hub = () => window.__fleetFake.hubs.get('primary');
  const row = (id) => hub().sessions.find((s) => s.id === id);
  const rowByName = (host, name) => hub().sessions.find((s) => s.host_alias === host && s.tmux_name === name);
  let rv = 1000;
  /** Patch a row on the fake hub and deliver `session:updated` (events.ts). */
  window.__setRow = (id, patch) => {
    const r = row(id);
    Object.assign(r, patch, { row_version: ++rv });
    window.__fleetFake.emit('session:updated', structuredClone(r));
    return r;
  };
  window.__D1A = window.__D1A || {};

  // ── conversations (conversation.ts Conversation / ConvTurn / ConvItem) ──
  const tool = (id, name, summary, target, startAgo, endAgo, extra = {}) => ({ kind: 'tool', id, name, summary, target, at: iso(startAgo), ended_at: endAgo == null ? null : iso(endAgo), done: endAgo != null, ...extra });
  const T = (prompt, startAgo, endAgo, items, over = {}) => ({ prompt, at: iso(startAgo), ended_at: endAgo == null ? null : iso(endAgo), items, prompt_uuid: 'u-' + startAgo, author: { name: 'Martin', person_id: 1, mine: true }, ...over });

  const EARLIER = (ago) => [
    T('Read docs/architecture.md and src/lib/inbox.ts and tell me how the Inbox decides what to list.', ago + 1800, ago + 1500, [
      tool('t1', 'Read', 'Read(docs/architecture.md)', 'docs/architecture.md', ago + 1795, ago + 1794),
      tool('t2', 'Read', 'Read(src/lib/inbox.ts)', 'src/lib/inbox.ts', ago + 1790, ago + 1789),
      tool('t3', 'Grep', 'Grep(countsTowardBadge)', 'countsTowardBadge', ago + 1785, ago + 1784),
      { kind: 'text', text: "The Inbox lists the rows whose **attention state** raises the badge (`countsTowardBadge`): *Action required*, *Failed* and *Blocked*. Everything else is counted on one quiet line under the list.\n\n- `inboxRows()` filters with `attentionState(s, opts)` and orders with `byTriage`.\n- Jev's *probably waiting* rows are kept apart (`proposedRows`).\n- `inboxGroupBy` is a pref: `'none'` or `'state'`." },
    ]),
  ];

  const CONV = {};
  // 1 working mid-turn with a subagent running and an edit with a diff
  CONV[1] = { turns: [...EARLIER(3600),
    T('FLT-212: group the Inbox by attention state, keep the single queue as an option, and add tests for both', 420, null, [
      { kind: 'text', text: "I'll start by mapping every caller of `inboxGroupBy`, then change the grouping in `Sidebar.svelte`." },
      tool('t10', 'Read', 'Read(src/lib/row_groups.ts)', 'src/lib/row_groups.ts', 410, 409),
      tool('t11', 'Bash', 'Bash(pnpm exec vitest run src/lib/inbox.test.ts)', 'pnpm exec vitest run src/lib/inbox.test.ts', 400, 380),
      { kind: 'text', text: 'All 41 inbox tests pass before the change. Editing the grouping now:' },
      tool('t12', 'Edit', 'Edit(src/lib/inbox.ts)', 'src/lib/inbox.ts', 300, 299),
      { kind: 'text', text: "```ts\nexport const inboxGroupBy = writable<InboxGroup>(readPref('inbox.group', 'state', isInboxGroup));\n```\n\nThe default is now **state**; `none` keeps the single queue." },
      { kind: 'subagent', id: 'sa1', name: 'Task', agent_type: 'Explore', description: 'Find every inbox grouping caller', result: null, error: false, at: iso(120), ended_at: null, done: false },
    ]),
  ], truncated: false, context: { tokens: 82000, window: 200000, pct: 41, stale: false }, events: [] };

  const blockedConv = (prompt, startAgo, items) => ({ turns: [T(prompt, startAgo, null, items)], truncated: false, context: { tokens: 116000, window: 200000, pct: 58, stale: false }, events: [] });
  CONV[2] = blockedConv('BILL-88: retry failed Stripe webhooks with exponential backoff, then push', 3000, [
    tool('t20', 'Read', 'Read(app/webhooks/stripe.py)', 'app/webhooks/stripe.py', 2990, 2989),
    tool('t21', 'Edit', 'Edit(app/webhooks/stripe.py)', 'app/webhooks/stripe.py', 2900, 2899),
    tool('t22', 'Bash', 'Bash(pytest tests/webhooks -q)', 'pytest tests/webhooks -q', 600, 560),
    { kind: 'text', text: '`pytest tests/webhooks` passes: **23 passed in 4.1s**. Retries now back off 1 s → 2 s → 4 s → … capped at 5 minutes, and a webhook that still fails after 8 attempts goes to the dead-letter table.\n\nCommitting and pushing.' },
    tool('t23', 'Bash', 'Bash(git commit -am "BILL-88: retry failed Stripe webhooks with backoff")', 'git commit -am "BILL-88: retry failed Stripe webhooks with backoff"', 300, 299),
    tool('t24', 'Bash', 'Bash(git push origin main)', 'git push origin main', 240, null),
  ]);
  CONV[3] = blockedConv('FLT-207: let the Android notification approve or deny a permission prompt', 7200, [
    tool('t30', 'Read', 'Read(app/src/main/java/dev/fleet/push/PushService.kt)', 'PushService.kt', 7100, 7099),
    { kind: 'text', text: 'There are three ways to deliver the answer from the notification. Each has a trade-off:\n\n| Approach | Works offline | Latency |\n|---|---|---|\n| Direct reply over the hub socket | no | ~200 ms |\n| WorkManager queue | yes | seconds |\n| Open the app | yes | a tap |' },
    tool('t31', 'AskUserQuestion', 'AskUserQuestion(Which approach…)', null, 660, null),
  ]);
  CONV[4] = blockedConv('FLT-241: write migration 208 and backfill the columns it adds', 10800, [
    tool('t40', 'Bash', 'Bash(git ls-tree --name-only origin/main crates/fleet-core/migrations/ | tail -1)', null, 10700, 10699),
    { kind: 'text', text: 'Main\'s last migration is `207_prompt_suggestion.sql`, so this one is **208**. Three columns could be backfilled from existing data; backfilling is a full-table write on `sessions` (~40k rows on mercury).' },
    tool('t41', 'AskUserQuestion', 'AskUserQuestion(Which tables…)', null, 1380, null),
  ]);
  CONV[5] = blockedConv('BILL-91: render the invoice PDF with the new footer', 14400, [
    tool('t50', 'Edit', 'Edit(app/invoices/templates/footer.html)', 'app/invoices/templates/footer.html', 2400, 2399),
    { kind: 'text', text: 'The footer now has the company address and the IBAN. It also needs a VAT ID, which is not in `settings.py` or in the fixtures.' },
    tool('t51', 'AskUserQuestion', 'AskUserQuestion(What VAT ID…)', null, 2280, null),
  ]);
  CONV[6] = { turns: [T('FLT-198: make the hub client back off and reconnect, then deploy to staging', 4000, 3120, [
    tool('t60', 'Edit', 'Edit(crates/fleet-core/src/hub_client.rs)', 'crates/fleet-core/src/hub_client.rs', 3900, 3899),
    tool('t61', 'Bash', 'Bash(cargo fleet-test -- hub_client::)', 'cargo fleet-test -- hub_client::', 3800, 3500),
    { kind: 'text', text: 'Backoff is in: 1 s, doubling, capped at 30 s, with ±20 % jitter. **14 tests pass.** PR #482 is open.\n\nBefore I deploy I need a few answers — I sent you a form.' },
    tool('t62', 'mcp__fleet__ask', 'mcp__fleet__ask(form: Deploy the hub build)', null, 3130, 3125),
  ])], truncated: false, context: { tokens: 124000, window: 200000, pct: 62, stale: false }, events: [] };
  CONV[7] = { turns: [T('FLT-219: measure push-to-talk latency end to end and cut it below 300 ms', 5400, 3900, [
    tool('t70', 'Bash', 'Bash(tools/voice/bench.sh --runs 20)', 'tools/voice/bench.sh --runs 20', 5300, 4800),
    { kind: 'text', text: 'Median 412 ms, p95 655 ms. Most of it is the WAV encode before upload. Trying streaming Opus frames instead.' },
    tool('t71', 'Edit', 'Edit(src/lib/voice/capture.ts)', 'src/lib/voice/capture.ts', 4000, 3999),
    { kind: 'text', text: 'API Error: 529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}' },
  ])], truncated: false, context: { tokens: 142000, window: 200000, pct: 71, stale: false }, events: [] };
  const stuckConv = (p, a, e) => ({ turns: [T(p, a, e, [{ kind: 'text', text: 'Working on it.' }, tool('ts' + a, 'Bash', 'Bash(cargo fleet-fast-check)', 'cargo fleet-fast-check', a - 20, a - 40)])], truncated: false, context: null, events: [] });
  CONV[8] = stuckConv('FLT-230: keep 10k lines of scrollback per pane without blowing memory', 1500, 1200);
  CONV[9] = stuckConv('BILL-95: fix half-even rounding on multi-currency tax lines', 11000, 10800);
  CONV[11] = stuckConv('FLT-233: queue answers while offline and replay them in order', 7400, 7200);
  CONV[12] = stuckConv('Run scripts/hub-e2e.sh in a loop until the flake shows up, then bisect', 8000, 7800);
  CONV[13] = { turns: [T('Add partial refunds to the billing API and the admin UI', 9000, 5400, [
    tool('t130', 'Edit', 'Edit(app/refunds/service.py)', 'app/refunds/service.py', 8000, 7999),
    { kind: 'text', text: "5-hour limit reached ∙ resets 8:20pm" },
  ])], truncated: false, context: null, events: [] };
  CONV[14] = { turns: [
    T('FLT-201: add fuzzy search to Settings, with keyboard focus on ⌘F', 5400, 900, [
      tool('t140', 'Read', 'Read(src/lib/Settings.svelte)', 'src/lib/Settings.svelte', 5300, 5299),
      tool('t141', 'Edit', 'Edit(src/lib/settings_search.ts)', 'src/lib/settings_search.ts', 4000, 3999),
      tool('t142', 'Bash', 'Bash(pnpm exec vitest run src/lib/settings_search.test.ts)', null, 3000, 2950),
      tool('t143', 'Bash', 'Bash(gh pr create --fill)', 'gh pr create --fill', 1200, 1190),
      { kind: 'text', text: '## Done\n\nSettings has a search box now.\n\n- **Fuzzy match** over every setting\'s label, help text and key (`settings_search.ts`)\n- `⌘F` focuses it; `Esc` clears it\n- 18 new tests, all green\n\nPR: https://github.com/martin-janci/claude-fleet/pull/479 — CI is **passing** and Ada approved it.' },
    ]),
  ], truncated: false, context: { tokens: 132000, window: 200000, pct: 66, stale: false }, events: [] };
  CONV[15] = { turns: [T('Fix the home-screen widget crash on Android 16 and open a PR', 12000, 10800, [
    tool('t150', 'Edit', 'Edit(app/src/main/java/dev/fleet/widget/FleetWidget.kt)', 'FleetWidget.kt', 11800, 11799),
    { kind: 'text', text: 'Fixed the `RemoteViews` crash (a null `PendingIntent` on Android 16). PR #131 is open.' },
    { kind: 'harness', tag: 'ci-monitor-event', body: 'PR #131 checks failed: rust / test (ubuntu-latest), frontend / vitest' },
  ])], truncated: false, context: null, events: [] };
  CONV[17] = { turns: [T('Draft the dunning email sequence for failed card payments', 2400, 1980, [
    { kind: 'text', text: "Here is a three-email sequence:\n\n1. **Day 0** — *Your payment didn't go through* (friendly, one-click update link)\n2. **Day 3** — *Action needed to keep your account*\n3. **Day 7** — *Last reminder before we pause your plan*\n\nShould the day-7 email pause the account automatically, or only warn and leave the pause to a person on the billing team?" },
  ])], truncated: false, context: null, events: [] };
  CONV[21] = { turns: [T('Add a federation e2e scenario for a hub that goes offline mid-session', 3600, null, [
    tool('t210', 'Read', 'Read(e2e/federation/scenarios.mjs)', 'e2e/federation/scenarios.mjs', 3500, 3499),
    tool('t211', 'Bash', 'Bash(node scripts/federation-e2e.mjs --only offline)', 'node scripts/federation-e2e.mjs --only offline', 60, null),
  ])], truncated: false, context: { tokens: 186000, window: 200000, pct: 93, stale: false }, events: [] };
  CONV[23] = { turns: [T('Export the ledger to CSV for the accountants, one file per month', 4200, 960, [tool('t230', 'Bash', 'Bash(python -m ledger.export --month 2026-10)', null, 1000, null)])], truncated: false, context: null, events: [] };
  CONV[24] = blockedConv('Rewrite the pricing page for the new seat tiers', 3600, [tool('t240', 'Edit', 'Edit(docs/pricing.md)', 'docs/pricing.md', 540, null)]);
  CONV[20] = { turns: [T('Find every caller of inboxGroupBy and list what each one expects', 360, null, [
    tool('t200', 'Grep', 'Grep(inboxGroupBy)', 'inboxGroupBy', 300, 299),
    { kind: 'text', text: 'Found 6 references in 4 files:\n\n- `src/lib/inbox.ts` — defines it\n- `src/lib/Sidebar.svelte` — reads it for the section list\n- `src/lib/SidebarFilters.svelte` — the Group select\n- `src/lib/inbox.test.ts` — 3 tests' },
    tool('t201', 'Read', 'Read(src/lib/SidebarFilters.svelte)', 'src/lib/SidebarFilters.svelte', 30, null),
  ], { author: { name: 'Inbox grouping', mine: false } })], truncated: false, context: { tokens: 18000, window: 200000, pct: 9, stale: false }, events: [] };
  CONV[25] = { turns: [T('Make CI warm the sccache bucket before the matrix starts', 2400, null, [
    tool('t250', 'Read', 'Read(.buildkite/pipeline.yml)', '.buildkite/pipeline.yml', 2300, 2299),
    { kind: 'text', text: 'Adding a `warm-cache` step that runs `cargo fleet-check` once and uploads the sccache bucket before the matrix fans out.' },
    tool('t251', 'Bash', 'Bash(cargo fleet-check)', 'cargo fleet-check', 30, null),
  ], { author: { name: 'Ada', person_id: 2, mine: false } })], truncated: false, context: { tokens: 102000, window: 200000, pct: 51, stale: false }, events: [] };
  CONV[22] = { turns: [T('Run the checkout load test at 500 rps for 10 minutes', 10800, null, [tool('t220', 'Bash', 'Bash(k6 run load/checkout.js)', 'k6 run load/checkout.js', 8400, null)])], truncated: false, context: null, events: [] };
  CONV[24].turns[0].author = { name: 'Ada', person_id: 2, mine: false };
  CONV[16] = { turns: [T('Proofread docs/hub.md against the current verdict table', 26 * 3600 + 900, 26 * 3600, [
    tool('t160', 'Read', 'Read(docs/hub.md)', 'docs/hub.md', 26 * 3600 + 880, 26 * 3600 + 879),
    tool('t161', 'Read', 'Read(src-tauri/src/backend/verdicts.rs)', 'src-tauri/src/backend/verdicts.rs', 26 * 3600 + 860, 26 * 3600 + 859),
    { kind: 'text', text: 'docs/hub.md matches the verdict table except for two links:\n\n1. `#refusals` should be `#refusal-table`\n2. `E_HUB_CONTRACT` is described twice\n\nI fixed both; nothing else changed.' },
  ])], truncated: false, context: { tokens: 36000, window: 200000, pct: 18, stale: false }, events: [] };
  window.__D1A.CONV = CONV;

  const DETAIL = {
    t12: { id: 't12', name: 'Edit', input: '{\n  "file_path": "src/lib/inbox.ts",\n  "old_string": "…",\n  "new_string": "…"\n}', command: null, result: 'The file src/lib/inbox.ts has been updated.', is_error: false,
      edit: { file_path: 'src/lib/inbox.ts', old: "export type InboxGroup = 'none';\nexport const inboxGroupBy = writable<InboxGroup>('none');\n\nexport function inboxSections(rows: SessionRow[]) {\n  return [{ key: 'all', label: null, rows }];\n}", new: "export type InboxGroup = 'none' | 'state';\nexport const inboxGroupBy = writable<InboxGroup>(readPref('inbox.group', 'state', isInboxGroup));\ninboxGroupBy.subscribe((g) => writePref('inbox.group', g));\n\nexport function inboxSections(rows: SessionRow[], group: InboxGroup) {\n  if (group === 'none') return [{ key: 'all', label: null, rows }];\n  return groupRows(rows, 'state');\n}" } },
    t11: { id: 't11', name: 'Bash', input: '{"command":"pnpm exec vitest run src/lib/inbox.test.ts"}', command: 'pnpm exec vitest run src/lib/inbox.test.ts', result: ' ✓ src/lib/inbox.test.ts (41 tests) 212ms\n\n Test Files  1 passed (1)\n      Tests  41 passed (41)\n   Duration  1.94s', is_error: false, edit: null },
    t24: { id: 't24', name: 'Bash', input: '{"command":"git push origin main"}', command: 'git push origin main', result: null, is_error: false, edit: null },
  };
  window.__D1A.DETAIL = DETAIL;

  // ── session_activity (conversation.ts ActivityProbe): the pane as the row says ──
  function probe(r) {
    return { claude_status: r.claude_status, current_activity: r.current_activity, stuck_kind: r.stuck_kind,
      waiting_for: r.pending_input ? r.pending_input.kind : null,
      spinner: r.claude_status === 'working' ? '✻ Cooking… (2m 14s · ↓ 3.1k tokens · esc to interrupt)' : null,
      pending_input: r.pending_input ? structuredClone(r.pending_input) : null };
  }

  const FORM = {
    form_id: 'f_deploy_198', session_id: 6, host_alias: 'htz-1', title: 'Deploy the hub build', why: 'PR #482 is green; I need a target and a window before I deploy', state: 'pending', created_at: NOW - 3125,
    spec: { spec: 'fleet.form/1', title: 'Deploy the hub build', intro: 'PR #482 (hub client backoff) passed CI. Pick where and when it goes out.', submit: 'Deploy', save_later: true,
      steps: [
        { title: 'Target', fields: [
          { name: 'env', type: 'select', label: 'Environment', required: true, options: [{ value: 'staging', label: 'Staging (fleet-stg.janci.dev)', proposed: { by: 'rule', reason: 'every hub build goes to staging first' } }, { value: 'prod', label: 'Production (fleet.janci.dev)' }] },
          { name: 'window', type: 'select', label: 'When', required: true, options: [['now', 'Now'], ['tonight', 'Tonight 22:00'], ['manual', 'I will trigger it']] },
          { name: 'notify', type: 'bool', label: 'Tell the team in #fleet when it is out', value: true },
        ] },
        { title: 'Notes', fields: [{ name: 'notes', type: 'textarea', label: 'Release note', placeholder: 'One line for the changelog', max_len: 280, value: 'Hub client backs off (1 s → 30 s, jitter) and reconnects by itself.', drafted: { by: 'claude', from: 'PR #482' } }] },
      ] } };

  const usage = [
    { account_uuid: 'acc-work', usage: { five_hour: { utilization: 100, resets_at: NOW + 38 * 60 }, seven_day: { utilization: 71, resets_at: NOW + 3 * 86400 }, seven_day_opus: null, seven_day_sonnet: null }, subscription: 'team', fetched_at: NOW - 120, source_host: 'mercury', status: 'ok', detail: null, next_try_at: 0 },
    { account_uuid: 'acc-admin', usage: { five_hour: { utilization: 46, resets_at: NOW + 3 * 3600 }, seven_day: { utilization: 63, resets_at: NOW + 2 * 86400 }, seven_day_opus: null, seven_day_sonnet: null }, subscription: 'max_20x', fetched_at: NOW - 120, source_host: 'mac', status: 'ok', detail: null, next_try_at: 0 },
  ];

  window.__D1A.usage = usage;
  /** The hub pushes usage as `account_usage:updated` (App.svelte: a paired desktop never calls list_account_usage). */
  window.__D1A.pushUsage = () => { for (const u of usage) window.__fleetFake.emit('account_usage:updated', structuredClone(u)); };
  const sends = (window.__D1A.sends = []);
  /** After an answer key the pane moves on: the row goes working (what the hub's next tick would write). */
  window.__D1A.autoAdvance = true;
  const linkOf = (r, over) => ({ link_id: 42, link_version: 3, state: 'active', primary: true, session_id: r.id, name: r.friendly_name || r.tmux_name, host: r.host_alias, source: 'started', strength: 'explicit', rule: null, why: `branch ${r.worktree_key || ''} · R3`, created_at: NOW - 86400, decided_at: NOW - 86000, ended_at: null, end_reason: null, claude_status: r.claude_status, needs_you: r.claude_status === 'blocked', archived: false, resumable: true, branch: r.worktree_key, pr_url: r.pr_url, cross_org: false, other_tasks: 0, ...over });
  const tref = (id, key, title, status = 'in_progress', name = 'In Progress') => ({ task_id: id, key, title, kind: 'tracker', status_category: status, status_name: name, url: null, unavailable: false, org_id: 1, tracker_name: 'Fleet tickets' });
  window.__FLEET_EXTRA__ = {
    session_presence: (a) => ({ session_id: a.args.session_id, heartbeat_secs: 30, viewers: [{ person_id: 1, name: 'Martin', device: 'MacBook Pro', since: NOW - 60, you: true }, ...(a.args.session_id === 2 ? [{ person_id: 2, name: 'Ada', device: 'Pixel 9', since: NOW - 180 }] : [])] }),
    session_access: () => [],
    queued_prompts: () => [],
    shell_terminals: (a) => ({ session_id: a.args.session_id, host_alias: row(a.args.session_id)?.host_alias ?? 'mac', terminals: [] }),
    touch_session_viewed: (a) => { const r = row(a.args.session_id); if (!r) return null; r.last_viewed_at = Math.floor(Date.now() / 1000); r.row_version = ++rv; return structuredClone(r); },
    get_fleet_settings: () => ({ 'work.prompt_tune': 'true', 'work.context_help': 'true', 'health.context_red_pct': '85' }),
    session_prompt_tune: (a) => ({ prompt: 'Proofread docs/hub.md against the verdict table in src-tauri/src/backend/verdicts.rs. Fix every link that points at a removed anchor and every refusal code the table no longer has. Do not reword anything else. Done when `REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen` leaves docs/hub.md unchanged.', notes: '- Named the setting to read\n- Said what to do when it is missing\n- Added what done looks like', unchanged: false, model: 'haiku', host_alias: 'mercury', at: Math.floor(Date.now() / 1000) }),
    pick_attachments: () => [{ path: '/Users/martin/Desktop/invoice-footer-mock.png', name: 'invoice-footer-mock.png', size: 184332, kind: 'image' }, { path: '/Users/martin/dev/notes/vat.md', name: 'vat.md', size: 2210, kind: 'text' }],
    attachment_preview: () => null,
    work_session_tasks: (a) => { const r = row(a.args.session_id); if (!r) return null; const key = r.work?.display_key || 'FLT-212'; return { session_id: r.id, org_id: 1, primary_link_id: 42, links: [
      { ...linkOf(r), task: tref('item:12', key, r.work?.title || 'Inbox: group needs-you rows by state') },
      { ...linkOf(r, { link_id: 44, state: 'suggested', primary: false, strength: 'weak', rule: 'R6', why: 'mentioned FLT-188 in a prompt · R6' }), task: tref('item:14', 'FLT-188', 'Inbox: keyboard triage', 'todo', 'To Do') },
      { ...linkOf(r, { link_id: 40, state: 'ended', primary: false, ended_at: NOW - 3 * 86400, end_reason: 'done' }), task: tref('item:9', 'FLT-170', 'Rail badge counts', 'done', 'Done') },
    ] }; },
    session_history: (a) => { const id = a.args.session_id; return [
      { id: 9, session_id: id, at: NOW - 240, kind: 'notification', detail: 'Claude needs your permission to use Bash', claude_session_id: null },
      { id: 8, session_id: id, at: NOW - 3000, kind: 'prompt', detail: row(id)?.last_prompt ?? null, claude_session_id: null },
      { id: 7, session_id: id, at: NOW - 3600 * 5, kind: 'session_start', detail: 'startup', claude_session_id: null },
    ]; },
    my_grants: () => ({ person_id: 1, grants: [
      { session_id: 24, level: 'answer', shared_by: 2, shared_by_name: 'Ada', granted_at: NOW - 86400, via_org: null },
      { session_id: 25, level: 'watch', shared_by: 2, shared_by_name: 'Ada', granted_at: NOW - 3 * 86400, via_org: null },
    ], requests: [] }),
    list_people: () => [{ id: 1, name: 'martin', display_name: 'Martin' }, { id: 2, name: 'ada', display_name: 'Ada' }],
    list_account_usage: () => structuredClone(usage),
    session_activity: (a) => { const r = row(a.args.session_id); return r ? probe(r) : null; },
    session_conversation: (a) => structuredClone(CONV[a.args.session_id] ?? { turns: [], truncated: false, context: null, events: [] }),
    session_conversations: (a) => { const r = row(a.args.session_id); return r && r.claude_session_id ? [{ id: a.args.session_id, session_id: a.args.session_id, claude_session_id: r.claude_session_id, transcript_path: null, started_at: r.started_at ?? NOW - 7200, ended_at: null, start_source: 'fleet', end_reason: null, model: r.model, first_prompt: r.last_prompt, turns: r.turn_seq, compactions: 0, current: true }] : []; },
    session_tool_detail: (a) => DETAIL[a.args.tool_use_id] ?? { id: a.args.tool_use_id, name: 'Bash', input: '{}', command: null, result: 'ok', is_error: false, edit: null },
    get_form: () => structuredClone(FORM),
    quick_replies: () => [
      { label: 'Continue', text: 'continue', auto_send: true },
      { label: 'Run the tests', text: 'Run the tests for what you changed and fix what fails.', auto_send: false },
      { label: 'Open a PR', text: 'Commit, push the branch and open a PR with a short description.', auto_send: false },
      { label: 'Explain', text: 'Explain what you changed and why, in five bullet points.', auto_send: false },
    ],
    send_prompt: async (a) => {
      sends.push(structuredClone(a));
      await new Promise((res) => setTimeout(res, 350)); // a real round trip to the host
      const r = rowByName(a.args.host_alias, a.args.tmux_name);
      if (r && window.__D1A.autoAdvance) {
        const key = a.args.keys;
        if (key && r.pending_input && !r.pending_input.multi) {
          setTimeout(() => window.__setRow(r.id, { claude_status: 'working', pending_input: null, current_activity: 'Thinking…', idle_since: null }), 1500);
        } else if (key && r.pending_input && r.pending_input.multi && /^\d$/.test(key)) {
          const pi = structuredClone(r.pending_input);
          const o = pi.options.find((x) => x.n === Number(key)); if (o) o.checked = !o.checked;
          window.__setRow(r.id, { pending_input: pi });
        } else if (!key) {
          setTimeout(() => window.__setRow(r.id, { claude_status: 'working', pending_input: null, last_prompt: a.args.prompt.slice(0, 200), prompt_submit_seq: (r.prompt_submit_seq ?? 0) + 1, current_activity: 'Thinking…', idle_since: null }), 1200);
        }
      }
      return null;
    },
    pty_open: () => 1,
    pty_drain: () => {
      const q = window.__D1A.ptyQueue;
      if (q && q.length) { const d = q.shift(); return { data: d, bytes: d.length, eof: false, overflowed: false }; }
      return { data: '', bytes: 0, eof: false, overflowed: false };
    },
  };
})();
