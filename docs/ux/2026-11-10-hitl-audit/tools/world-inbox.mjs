// The one consistent world for the d1a captures: Martin's paired hub, 4 hosts,
// 3 projects, ~24 sessions covering every attention state of attention.ts.
// Times are relative to the moment the world is built (seconds).
export function buildWorld({ variant = 'full' } = {}) {
  const NOW = Math.floor(Date.now() / 1000);
  const M = 60, H = 3600, D = 86400;
  const ME = 1;
  const own = { owner_person_id: ME, visibility: 'private', origin: 'person', origin_ref: '1', agent: 'claude' };

  const projects = [
    { project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/Users/martin/dev/claude-fleet', last_session_at: NOW - 2 * M, adopted: false, system: false, github_origin: 'https://github.com/martin-janci/claude-fleet' },
      worktrees: [
        { id: 11, project_id: 1, host_alias: 'local', name: 'FLT-212-inbox-grouping', path: '/Users/martin/dev/claude-fleet/.worktrees/FLT-212-inbox-grouping', branch: 'FLT-212-inbox-grouping' },
        { id: 12, project_id: 1, host_alias: 'local', name: 'FLT-198-hub-reconnect', path: '/Users/martin/dev/claude-fleet/.worktrees/FLT-198-hub-reconnect', branch: 'FLT-198-hub-reconnect' },
        { id: 13, project_id: 1, host_alias: 'local', name: 'FLT-230-terminal-scrollback', path: '/Users/martin/dev/claude-fleet/.worktrees/FLT-230-terminal-scrollback', branch: 'FLT-230-terminal-scrollback' },
        { id: 14, project_id: 1, host_alias: 'local', name: 'FLT-241-migration-208', path: '/Users/martin/dev/claude-fleet/.worktrees/FLT-241-migration-208', branch: 'FLT-241-migration-208' },
        { id: 15, project_id: 1, host_alias: 'local', name: 'FLT-219-voice-latency', path: '/Users/martin/dev/claude-fleet/.worktrees/FLT-219-voice-latency', branch: 'FLT-219-voice-latency' },
      ] },
    { project: { id: 2, owner: 'martin-janci', repo: 'fleet-mobile', base_path: '/Users/martin/dev/fleet-mobile', last_session_at: NOW - 6 * M, adopted: false, system: false, github_origin: 'https://github.com/martin-janci/fleet-mobile' },
      worktrees: [
        { id: 21, project_id: 2, host_alias: 'local', name: 'FLT-207-push-actions', path: '/Users/martin/dev/fleet-mobile/.worktrees/FLT-207-push-actions', branch: 'FLT-207-push-actions' },
        { id: 22, project_id: 2, host_alias: 'local', name: 'FLT-225-orbit-palette', path: '/Users/martin/dev/fleet-mobile/.worktrees/FLT-225-orbit-palette', branch: 'FLT-225-orbit-palette' },
        { id: 23, project_id: 2, host_alias: 'local', name: 'FLT-233-offline-queue', path: '/Users/martin/dev/fleet-mobile/.worktrees/FLT-233-offline-queue', branch: 'FLT-233-offline-queue' },
      ] },
    { project: { id: 3, owner: 'acme', repo: 'billing-api', base_path: '/home/martin/src/billing-api', last_session_at: NOW - 4 * M, adopted: false, system: false, github_origin: 'https://github.com/acme/billing-api' },
      worktrees: [
        { id: 31, project_id: 3, host_alias: 'local', name: 'BILL-88-stripe-webhook-retry', path: '/home/martin/src/billing-api/.worktrees/BILL-88-stripe-webhook-retry', branch: 'BILL-88-stripe-webhook-retry' },
        { id: 32, project_id: 3, host_alias: 'local', name: 'BILL-91-invoice-pdf', path: '/home/martin/src/billing-api/.worktrees/BILL-91-invoice-pdf', branch: 'BILL-91-invoice-pdf' },
        { id: 33, project_id: 3, host_alias: 'local', name: 'BILL-95-tax-rounding', path: '/home/martin/src/billing-api/.worktrees/BILL-95-tax-rounding', branch: 'BILL-95-tax-rounding' },
      ] },
  ];
  const wt = Object.fromEntries(projects.flatMap((p) => p.worktrees.map((w) => [w.id, w])));
  const ctx = (pct, model = 'claude-opus-5-5') => ({ model, context_pct: pct, context_tokens: Math.round(pct * 2000), context_window: 200000, context_source: 'transcript', context_at: NOW - 60, context_stale: false });
  const cost = (usd, model = 'claude-opus-5-5') => ({ usage_cost_micros: Math.round(usd * 1e6), usage_model: model, usage_input_tokens: Math.round(usd * 9000), usage_output_tokens: Math.round(usd * 2100), usage_cache_read_tokens: Math.round(usd * 410000), usage_cache_write_tokens: Math.round(usd * 30000), usage_updated_at: NOW - 60 });
  const pr = (n, repo, ci, over = {}) => ({ pr_url: `https://github.com/${repo}/pull/${n}`, ci_status: ci, pr_checked_at: NOW - 90,
    pr_evidence: { draft: false, state: 'OPEN', review_decision: 'REVIEW_REQUIRED', merge_state: ci === 'failing' ? 'BLOCKED' : 'CLEAN', ahead: 0, dirty: false,
      checks: { total: 9, pending: ci === 'pending' ? 3 : 0, skipped: 1, failing: ci === 'failing' ? [{ name: 'rust / test (ubuntu-latest)', url: `https://github.com/${repo}/actions/runs/1` }, { name: 'frontend / vitest' }] : [], failing_total: ci === 'failing' ? 2 : 0 }, ...over } });
  const work = (key, title, status = 'in_progress') => ({ work: { link_id: 100 + key.length, item_id: null, key, display_key: key, title, source: 'started', kind: 'tracker', status_category: status, effective_status: status, status_name: status === 'done' ? 'Done' : 'In Progress', url: null, state: 'confirmed' } });

  // Every session: [host, tmux, overrides]. Ages vary 2 m … 3 d.
  const S = [
    // 1 working, subagent running
    ['mac', 'cf-inbox-grouping', { ...own, friendly_name: 'Inbox grouping', project_id: 1, worktree_id: 11, worktree_key: 'FLT-212-inbox-grouping', claude_status: 'working',
      current_activity: 'Task(Explore: find every inbox grouping caller)', last_prompt: 'FLT-212: group the Inbox by attention state, keep the single queue as an option, and add tests for both', last_activity_at: NOW - 20, started_at: NOW - 3 * H, created_at: NOW - 3 * H,
      idle_since: null, last_turn_at: NOW - 40 * M, last_stop_at: NOW - 40 * M, turn_seq: 6, last_viewed_at: NOW - 30 * M, effort_level: 'high', ...ctx(41), ...cost(4.82), ...work('FLT-212', 'Inbox: group needs-you rows by state'), claude_session_id: 'a1f0c2d4-1111-4a7e-9b1c-5d0e7f3a2b01', tmux_pane_id: '%3' }],
    // 2 permission dialog
    ['mercury', 'billing-webhook-retry', { ...own, friendly_name: 'Stripe webhook retry', project_id: 3, worktree_id: 31, worktree_key: 'BILL-88-stripe-webhook-retry', claude_status: 'blocked',
      pending_input: { kind: 'permission', question: 'Do you want to proceed?', detail: 'Bash(git push origin main)', options: [{ n: 1, label: 'Yes', selected: true }, { n: 2, label: "Yes, and don't ask again for git push commands in /home/martin/src/billing-api", selected: false }, { n: 3, label: 'No, and tell Claude what to do differently (esc)', selected: false }] },
      current_activity: 'Waiting for permission: Bash(git push origin main)', last_prompt: 'BILL-88: retry failed Stripe webhooks with exponential backoff, then push', last_activity_at: NOW - 4 * M, idle_since: null, last_turn_at: NOW - 50 * M, last_stop_at: NOW - 50 * M, turn_seq: 4, ...ctx(58), ...cost(2.17), ...work('BILL-88', 'Retry failed Stripe webhooks'), claude_session_id: 'b2e1d3c5-2222-4b8f-8c2d-6e1f8a4b3c02' }],
    // 3 multiple choice
    ['mac', 'fm-push-actions', { ...own, friendly_name: 'Push notification actions', project_id: 2, worktree_id: 21, worktree_key: 'FLT-207-push-actions', claude_status: 'blocked',
      pending_input: { kind: 'input', question: 'Which approach should the notification action use to answer a permission prompt?', options: [{ n: 1, label: 'Direct reply over the hub websocket', selected: true }, { n: 2, label: 'Queue it in WorkManager and retry', selected: false }, { n: 3, label: 'Open the app on tap and answer there', selected: false }, { n: 4, label: 'Type something.', selected: false }, { n: 5, label: 'Chat about this', selected: false }] },
      current_activity: 'Waiting for your answer', last_prompt: 'FLT-207: let the Android notification approve or deny a permission prompt', last_activity_at: NOW - 11 * M, idle_since: null, last_turn_at: NOW - 2 * H, last_stop_at: NOW - 2 * H, turn_seq: 3, ...ctx(33, 'claude-sonnet-5'), ...cost(0.94, 'claude-sonnet-5'), ...work('FLT-207', 'Act on push notifications'), claude_session_id: 'c3f2e4d6-3333-4c9a-9d3e-7f2a9b5c4d03' }],
    // 4 multi-select
    ['htz-1', 'cf-migration-208', { ...own, friendly_name: 'Migration 208 cleanup', project_id: 1, worktree_id: 14, worktree_key: 'FLT-241-migration-208', claude_status: 'blocked',
      pending_input: { kind: 'input', multi: true, question: 'Which tables should migration 208 backfill?', options: [{ n: 1, label: 'sessions.turn_outcome', selected: true, checked: true }, { n: 2, label: 'work_links.state', selected: false, checked: false }, { n: 3, label: 'hosts.signed_out_logins', selected: false, checked: true }, { n: 4, label: 'Type something.', selected: false, checked: false }, { n: 5, label: 'Chat about this', selected: false }] },
      current_activity: 'Waiting for your answer', last_prompt: 'FLT-241: write migration 208 and backfill the columns it adds', last_activity_at: NOW - 23 * M, idle_since: null, last_turn_at: NOW - 3 * H, last_stop_at: NOW - 3 * H, turn_seq: 2, ...ctx(27), ...cost(1.31), ...work('FLT-241', 'Migration 208: backfill'), claude_session_id: 'd4a3f5e7-4444-4dab-8e4f-8a3b0c6d5e04' }],
    // 5 free text question (AskUserQuestion with only Type something)
    ['mercury', 'billing-invoice-pdf', { ...own, friendly_name: 'Invoice PDF layout', project_id: 3, worktree_id: 32, worktree_key: 'BILL-91-invoice-pdf', claude_status: 'blocked',
      pending_input: { kind: 'input', question: 'What VAT ID should the sample invoice footer show?', options: [{ n: 1, label: 'Type something.', selected: true }, { n: 2, label: 'Chat about this', selected: false }] },
      current_activity: 'Waiting for your answer', last_prompt: 'BILL-91: render the invoice PDF with the new footer', last_activity_at: NOW - 38 * M, idle_since: null, last_turn_at: NOW - 4 * H, last_stop_at: NOW - 4 * H, turn_seq: 5, ...ctx(49), ...cost(1.88), claude_session_id: 'e5b4a6f8-5555-4ebc-9f5a-9b4c1d7e6f05' }],
    // 6 pending chat form
    ['htz-1', 'cf-hub-reconnect', { ...own, friendly_name: 'Hub reconnect backoff', project_id: 1, worktree_id: 12, worktree_key: 'FLT-198-hub-reconnect', claude_status: 'idle',
      pending_form: { form_id: 'f_deploy_198', title: 'Deploy the hub build' }, last_prompt: 'FLT-198: make the hub client back off and reconnect, then deploy to staging', last_activity_at: NOW - 52 * M, idle_since: NOW - 52 * M, last_turn_at: NOW - 52 * M, last_stop_at: NOW - 52 * M, turn_seq: 9, last_viewed_at: NOW - 2 * H, ...ctx(62), ...cost(6.4), ...pr(482, 'martin-janci/claude-fleet', 'passing'), ...work('FLT-198', 'Hub client: back off and reconnect'), claude_session_id: 'f6c5b7a9-6666-4fcd-8a6b-0c5d2e8f7a06' }],
    // 7 failed turn (stop_failed)
    ['mac', 'cf-voice-latency', { ...own, friendly_name: 'Voice latency', project_id: 1, worktree_id: 15, worktree_key: 'FLT-219-voice-latency', claude_status: 'failed',
      current_activity: 'API Error: 529 {"type":"overloaded_error","message":"Overloaded"}', last_prompt: 'FLT-219: measure push-to-talk latency end to end and cut it below 300 ms', last_activity_at: NOW - 1 * H - 5 * M, idle_since: NOW - 65 * M, last_turn_at: NOW - 65 * M, last_stop_at: NOW - 65 * M, turn_seq: 7, ...ctx(71), ...cost(3.05), claude_session_id: 'a7d6c8b0-7777-40de-9b7c-1d6e3f9a8b07' }],
    // 8-12 stuck kinds
    ['htz-1', 'cf-scrollback', { ...own, friendly_name: 'Terminal scrollback', project_id: 1, worktree_id: 13, worktree_key: 'FLT-230-terminal-scrollback', claude_status: 'idle', stuck_kind: 'press_enter', stuck_since: NOW - 7 * M,
      last_prompt: 'FLT-230: keep 10k lines of scrollback per pane without blowing memory', last_activity_at: NOW - 7 * M, idle_since: NOW - 7 * M, last_turn_at: NOW - 20 * M, last_stop_at: NOW - 20 * M, turn_seq: 3, ...ctx(38), ...cost(1.02), claude_session_id: 'b8e7d9c1-8888-41ef-8c8d-2e7f4a0b9c08' }],
    ['mercury', 'billing-tax-rounding', { ...own, friendly_name: 'Tax rounding', project_id: 3, worktree_id: 33, worktree_key: 'BILL-95-tax-rounding', claude_status: 'idle', stuck_kind: 'auth_menu', stuck_since: NOW - 18 * M,
      last_prompt: 'BILL-95: fix half-even rounding on multi-currency tax lines', last_activity_at: NOW - 18 * M, idle_since: NOW - 18 * M, last_turn_at: NOW - 3 * H, last_stop_at: NOW - 3 * H, turn_seq: 2, ...ctx(12), ...cost(0.41), claude_session_id: 'c9f8e0d2-9999-42f0-9d9e-3f8a5b1c0d09' }],
    ['htz-1', 'fm-orbit-palette', { ...own, friendly_name: 'Orbit palette port', project_id: 2, worktree_id: 22, worktree_key: 'FLT-225-orbit-palette', claude_status: 'idle', stuck_kind: 'trust_prompt', stuck_since: NOW - 27 * M,
      last_prompt: null, last_activity_at: NOW - 27 * M, idle_since: NOW - 27 * M, last_turn_at: null, last_stop_at: null, turn_seq: 0, started_at: NOW - 28 * M, created_at: NOW - 28 * M, ...ctx(0), claude_session_id: null }],
    ['mac', 'fm-offline-queue', { ...own, friendly_name: 'Offline queue', project_id: 2, worktree_id: 23, worktree_key: 'FLT-233-offline-queue', claude_status: 'idle', stuck_kind: 'reconnect', stuck_since: NOW - 44 * M,
      last_prompt: 'FLT-233: queue answers while offline and replay them in order', last_activity_at: NOW - 44 * M, idle_since: NOW - 44 * M, last_turn_at: NOW - 2 * H, last_stop_at: NOW - 2 * H, turn_seq: 4, ...ctx(44, 'claude-sonnet-5'), ...cost(0.77, 'claude-sonnet-5'), claude_session_id: 'd0a9f1e3-aaaa-43a1-8eaf-4a9b6c2d1e10' }],
    ['htz-1', 'cf-e2e-runner', { ...own, friendly_name: 'Hub e2e runner', project_id: 1, claude_status: 'idle', stuck_kind: 'oom', stuck_since: NOW - 2 * H,
      last_prompt: 'Run scripts/hub-e2e.sh in a loop until the flake shows up, then bisect', last_activity_at: NOW - 2 * H, idle_since: NOW - 2 * H, last_turn_at: NOW - 2 * H - 10 * M, last_stop_at: NOW - 2 * H - 10 * M, turn_seq: 11, ...ctx(80), ...cost(9.12), claude_session_id: 'e1b0a2f4-bbbb-44b2-9fb0-5b0c7d3e2f11' }],
    // 13 limit reached (account at its five-hour limit)
    ['mercury', 'billing-refunds', { ...own, friendly_name: 'Partial refunds', project_id: 3, claude_status: 'idle', account_uuid: 'acc-work',
      current_activity: null, last_prompt: 'Add partial refunds to the billing API and the admin UI', last_activity_at: NOW - 1 * H - 30 * M, idle_since: NOW - 90 * M, last_turn_at: NOW - 90 * M, last_stop_at: NOW - 90 * M, turn_seq: 8, last_viewed_at: NOW - 80 * M, ...ctx(55), ...cost(7.7), claude_session_id: 'f2c1b3a5-cccc-45c3-8a01-6c1d8e4f3a12' }],
    // 14 done, PR, green CI, unread
    ['mac', 'cf-settings-search', { ...own, friendly_name: 'Settings search', project_id: 1, claude_status: 'completed',
      last_prompt: 'FLT-201: add fuzzy search to Settings, with keyboard focus on ⌘F', last_activity_at: NOW - 14 * M, idle_since: NOW - 14 * M, last_turn_at: NOW - 14 * M, last_stop_at: NOW - 14 * M, turn_seq: 12, last_viewed_at: NOW - 1 * H, ...ctx(66), ...cost(5.9), ...pr(479, 'martin-janci/claude-fleet', 'passing', { review_decision: 'APPROVED' }), ...work('FLT-201', 'Settings: fuzzy search', 'in_progress'), claude_session_id: 'a3d2c4b6-dddd-46d4-9b12-7d2e9f5a4b13',
      prompt_suggestion: 'Merge the PR and delete the branch' }],
    // 15 done, red CI
    ['htz-1', 'fm-widget-crash', { ...own, friendly_name: 'Widget crash fix', project_id: 2, claude_status: 'idle',
      last_prompt: 'Fix the home-screen widget crash on Android 16 and open a PR', last_activity_at: NOW - 3 * H, idle_since: NOW - 3 * H, last_turn_at: NOW - 3 * H, last_stop_at: NOW - 3 * H, turn_seq: 6, last_viewed_at: NOW - 2 * H, ...ctx(52, 'claude-sonnet-5'), ...cost(2.6, 'claude-sonnet-5'), ...pr(131, 'martin-janci/fleet-mobile', 'failing'), claude_session_id: 'b4e3d5c7-eeee-47e5-8c23-8e3f0a6b5c14' }],
    // 16 idle (seen)
    ['mac', 'cf-docs-pass', { ...own, friendly_name: 'Docs pass', project_id: 1, claude_status: 'idle',
      last_prompt: 'Proofread docs/hub.md against the current verdict table', last_activity_at: NOW - 1 * D - 2 * H, idle_since: NOW - 26 * H, last_turn_at: NOW - 26 * H, last_stop_at: NOW - 26 * H, turn_seq: 2, last_viewed_at: NOW - 25 * H, ...ctx(18, 'claude-haiku-5'), ...cost(0.12, 'claude-haiku-5'), claude_session_id: 'c5f4e6d8-ffff-48f6-9d34-9f4a1b7c6d15' }],
    // 17 Jev: probably waiting (free text at end of turn)
    ['mercury', 'billing-dunning', { ...own, friendly_name: 'Dunning emails', project_id: 3, claude_status: 'idle', turn_outcome: 'asked',
      last_prompt: 'Draft the dunning email sequence for failed card payments', last_activity_at: NOW - 33 * M, idle_since: NOW - 33 * M, last_turn_at: NOW - 33 * M, last_stop_at: NOW - 33 * M, turn_seq: 3, last_viewed_at: NOW - 30 * M, ...ctx(21), ...cost(0.66), claude_session_id: 'd6a5f7e9-0000-49a7-8e45-0a5b2c8d7e16' }],
    // 18 lost (host reboot)
    ['htz-1', 'cf-bench-sqlite', { ...own, friendly_name: 'SQLite bench', project_id: 1, claude_status: 'idle', lost_at: NOW - 1 * D, lost_reason: 'host_reboot',
      last_prompt: 'Benchmark the migrated template copy against a fresh migrate', last_activity_at: NOW - 1 * D - 1 * H, idle_since: NOW - 1 * D - 1 * H, last_turn_at: NOW - 1 * D - 1 * H, last_stop_at: NOW - 1 * D - 1 * H, turn_seq: 5, last_viewed_at: NOW - 1 * D, ...ctx(47), ...cost(1.4), claude_session_id: 'e7b6a8f0-1212-4ab8-9f56-1b6c3d9e8f17' }],
    // 19 ghost
    ['mac', 'old-spike-tauri3', { ...own, claude_status: 'idle', status: 'ghost', project_id: 1,
      last_prompt: 'Spike: what breaks on Tauri 3 alpha?', last_activity_at: NOW - 3 * D, idle_since: NOW - 3 * D, last_turn_at: NOW - 3 * D, last_stop_at: NOW - 3 * D, turn_seq: 4, last_viewed_at: NOW - 3 * D, ...ctx(30), ...cost(0.9) }],
    // 20 background agent
    ['mac', 'bg:7f3a91', { ...own, kind: 'bg', origin: 'background', origin_ref: '1', friendly_name: 'Explore inbox callers', project_id: 1, parent_session_id: 1, claude_status: 'working',
      current_activity: 'Grep(inboxGroupBy)', last_prompt: 'Find every caller of inboxGroupBy and list what each one expects', claude_session_id: '7f3a91c2-2121-4d10-9e00-3a7f91c2bb20', last_activity_at: NOW - 15, started_at: NOW - 6 * M, created_at: NOW - 6 * M, idle_since: null, last_turn_at: null, last_stop_at: null, turn_seq: 0, ...ctx(9, 'claude-haiku-5'), ...cost(0.08, 'claude-haiku-5') }],
    // 21 working with full context
    ['htz-1', 'cf-federation-tests', { ...own, friendly_name: 'Federation e2e', project_id: 1, claude_status: 'working',
      current_activity: 'Bash(node scripts/federation-e2e.mjs --only offline)', last_prompt: 'Add a federation e2e scenario for a hub that goes offline mid-session', last_activity_at: NOW - 40, idle_since: null, last_turn_at: NOW - 1 * H, last_stop_at: NOW - 1 * H, turn_seq: 14, last_viewed_at: NOW - 50 * M, ...ctx(93), ...cost(11.3), claude_session_id: 'f8c7b9a1-1313-4bc9-8a67-2c7d4e0f9a18' }],
    // 22 host down
    ['oci-arm', 'billing-load-test', { ...own, friendly_name: 'Load test', project_id: 3, claude_status: 'working',
      current_activity: 'Bash(k6 run load/checkout.js)', last_prompt: 'Run the checkout load test at 500 rps for 10 minutes', claude_session_id: '9b1c2d3e-2222-4f00-8a22-22a22a22a222', last_activity_at: NOW - 2 * H - 20 * M, idle_since: null, last_turn_at: NOW - 3 * H, last_stop_at: NOW - 3 * H, turn_seq: 2, ...ctx(24), ...cost(0.5) }],
    // 23 stale working
    ['mercury', 'billing-ledger-export', { ...own, friendly_name: 'Ledger export', project_id: 3, claude_status: 'idle', stale_working_at: NOW - 16 * M,
      last_prompt: 'Export the ledger to CSV for the accountants, one file per month', last_activity_at: NOW - 16 * M, idle_since: NOW - 16 * M, last_turn_at: NOW - 70 * M, last_stop_at: NOW - 70 * M, turn_seq: 3, last_viewed_at: NOW - 60 * M, ...ctx(35), ...cost(0.95) }],
    // 24 shared with me (Ada's), answerable
    ['mercury', 'ada-pricing-page', { owner_person_id: 2, visibility: 'private', origin: 'person', origin_ref: '2', friendly_name: 'Pricing page copy', project_id: 3, claude_status: 'blocked',
      pending_input: { kind: 'permission', question: 'Do you want to make this edit to pricing.md?', detail: 'Edit(docs/pricing.md)', options: [{ n: 1, label: 'Yes', selected: true }, { n: 2, label: 'Yes, allow all edits during this session (shift+tab)', selected: false }, { n: 3, label: 'No, and tell Claude what to do differently (esc)', selected: false }] },
      last_prompt: 'Rewrite the pricing page for the new seat tiers', claude_session_id: '9b1c2d3e-2424-4f00-8a24-24a24a24a224', last_activity_at: NOW - 9 * M, idle_since: null, last_turn_at: NOW - 1 * H, last_stop_at: NOW - 1 * H, turn_seq: 3, ...ctx(29), ...cost(0.7) }],
    // 25 shared with me, watch only
    ['htz-1', 'ada-ci-cache', { owner_person_id: 2, visibility: 'private', origin: 'person', origin_ref: '2', friendly_name: 'CI cache warmup', project_id: 1, claude_status: 'working',
      current_activity: 'Bash(cargo fleet-check)', last_prompt: 'Make CI warm the sccache bucket before the matrix starts', claude_session_id: '9b1c2d3e-2525-4f00-8a25-25a25a25a225', last_activity_at: NOW - 30, idle_since: null, last_turn_at: NOW - 40 * M, last_stop_at: NOW - 40 * M, turn_seq: 5, ...ctx(51), ...cost(2.2) }],
  ];
  const sessions = S.map(([host, name, o]) => {
    const w = o.worktree_id ? wt[o.worktree_id] : null;
    return { host, name, created_at: o.started_at ?? NOW - 5 * H, started_at: NOW - 5 * H, tmux_pane_id: '%1', ...o,
      ...(o.usage_cost_micros == null ? {} : {}), ...(w ? {} : {}) };
  });
  if (variant === 'calm') {
    // Nothing needs you: only working / idle / done-and-read rows.
    for (const s of sessions) {
      Object.assign(s, { claude_status: s.claude_status === 'working' ? 'working' : 'idle', pending_input: null, pending_form: null, stuck_kind: null, stuck_since: null, turn_outcome: null, stale_working_at: null, account_uuid: null, ci_status: s.ci_status === 'failing' ? 'passing' : s.ci_status, last_viewed_at: NOW });
      if (s.pr_evidence) s.pr_evidence = { ...s.pr_evidence, checks: { ...s.pr_evidence.checks, failing: [], failing_total: 0 } };
      if (s.context_pct > 80) s.context_pct = 60;
    }
  }
  const hosts = [
    { alias: 'mac', ssh_alias: null, transport: 'local', account_uuid: 'acc-admin', load_1m: 3.1 },
    { alias: 'mercury', account_uuid: 'acc-work', load_1m: 1.4 },
    { alias: 'htz-1', account_uuid: 'acc-admin', load_1m: 5.2 },
    variant === 'calm' ? { alias: 'oci-arm', reachable: true } : { alias: 'oci-arm', reachable: false, last_pinged_at: NOW - 2 * H - 15 * M },
  ];
  const accounts = [
    { uuid: 'acc-admin', email: 'martin@janci.dev', display_name: 'Martin', organization_name: 'Martin Janci', organization_uuid: 'org-1', seat_tier: 'max_20x', last_seen_at: NOW - 60, nickname: 'personal', has_extra_usage: false },
    { uuid: 'acc-work', email: 'martin@acme.example', display_name: 'Martin (Acme)', organization_name: 'Acme', organization_uuid: 'org-2', seat_tier: 'team', last_seen_at: NOW - 60, nickname: 'acme', has_extra_usage: false },
  ];
  return {
    NOW,
    world: {
      primary: { url: 'https://fleet.janci.dev', label: 'Own hub', hosts, sessions, projects, accounts,
        profile: { person_id: 1, name: 'martin', display_name: 'Martin', email: 'martin@janci.dev', is_owner: true, sso: [], env: [], git: null } },
      extras: [],
    },
  };
}
