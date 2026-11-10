// d1a UX-audit captures. Run from /home/user/claude-fleet:
//   node docs/ux/2026-11-10-hitl-audit/tools/capture-inbox.mjs [only-substring]   (from the repo root; screenshots go to $HITL_OUT or /tmp/hitl-capture)
import { capture } from './capture-lib.mjs';
import { buildWorld } from './world-inbox.mjs';
import { writeFileSync } from 'node:fs';

const D = process.env.HITL_OUT || '/tmp/hitl-capture';
const HERE = new URL('.', import.meta.url).pathname;
const EXTRA = HERE + 'extra-inbox.js';
const full = buildWorld().world;
const calm = buildWorld({ variant: 'calm' }).world;
const W1100 = { width: 1100, height: 720 };
const only = process.argv[2] ?? null;
const log = {};

// ── helpers ──
async function boot(t) {
  await t.settle(600);
  await t.fake(() => window.__D1A.pushUsage());
  await t.settle(700);
}
const park = (t) => t.page.mouse.move(1, 899);
function rowOf(t, name) {
  return t.page.getByTestId('sess-row').filter({ hasText: name }).first();
}
async function open(t, name, from = 'rail-inbox') {
  if (from) await t.click(from);
  const r = rowOf(t, name);
  await r.scrollIntoViewIfNeeded();
  // click near the status dot: the row's hover toolbar covers the middle and the name (see NOTES)
  await r.click({ position: { x: 14, y: 10 } });
  await park(t);
  await t.settle(1800);
}
async function groupBy(t, testid, value) {
  await t.page.getByTestId(testid).selectOption(value);
  await t.settle(500);
}
const m = (screen, state, reached) => ({ screen, state, reached });

const CONV_STATES = [
  ['a-working-subagent', 'Inbox grouping', 'working mid-turn, subagent running', 'rail-sessions'],
  ['b-permission', 'Stripe webhook retry', 'waiting on a permission dialog Bash(git push origin main)'],
  ['c-choice', 'Push notification actions', 'waiting on a multiple-choice question'],
  ['d-multiselect', 'Migration 208 cleanup', 'waiting on a multi-select question'],
  ['e-freetext', 'Invoice PDF layout', 'waiting on a free-text question (Type something.)'],
  ['f-form', 'Hub reconnect backoff', 'pending chat form'],
  ['g-failed', 'Voice latency', 'failed turn (API 529)'],
  ['h-stuck-press-enter', 'Terminal scrollback', 'stuck: press Enter'],
  ['i-limit', 'Partial refunds', 'account at its five-hour limit'],
  ['j-done-pr-green', 'Settings search', 'finished turn, PR #479 green CI, unread (not in the Inbox: Done is not counted)', 'rail-sessions'],
];

const runs = [];

// ── 2. Inbox ──
for (const [theme, vp] of [['dark', null], ['light', null], ['dark', W1100]]) {
  runs.push({ name: 'inbox', world: full, theme, viewport: vp ?? undefined, extraFile: EXTRA, async steps(t) {
    await boot(t);
    await t.click('rail-inbox'); await park(t);
    await t.shot('group-state', m('Inbox', 'populated, Group: State (default)', 'rail-inbox'));
    if (theme !== 'dark' || vp) return;
    await t.page.getByTestId('inbox').evaluate((e) => (e.scrollTop = e.scrollHeight)); await t.settle(300);
    await t.shot('group-state-scrolled-end', m('Inbox', 'scrolled to the end: Failed/Blocked sections, Jev proposed row, footer', 'rail-inbox; scroll list to bottom'));
    await t.page.getByTestId('inbox').evaluate((e) => (e.scrollTop = 0));
    await groupBy(t, 'inbox-group-select', 'none');
    await t.shot('group-none', m('Inbox', 'Group: None (single queue)', 'rail-inbox; Group select → None'));
    await t.page.getByTestId('inbox').evaluate((e) => (e.scrollTop = e.scrollHeight)); await t.settle(300);
    await t.shot('group-none-scrolled-end', m('Inbox', 'single queue scrolled to the end, Jev proposed row', 'Group None; scroll to bottom'));
    const prop = t.page.getByTestId('inbox-proposed');
    if (await prop.count()) await t.shotOf(prop, 'jev-proposed-element', m('Inbox', 'Jev "probably waiting" block (element)', 'Group None; scroll to bottom'));
  } });
}
for (const [theme, vp] of [['dark', null], ['light', null], ['dark', W1100]]) {
  runs.push({ name: 'inbox-empty', world: calm, theme, viewport: vp ?? undefined, extraFile: EXTRA, async steps(t) {
    await boot(t); await t.click('rail-inbox'); await park(t);
    await t.shot('nothing-needs-you', m('Inbox', 'empty: nothing needs you', 'calm world; rail-inbox'));
  } });
}

// ── 3. Conversation states ──
for (const theme of ['dark', 'light']) {
  runs.push({ name: 'conv', world: full, theme, extraFile: EXTRA, async steps(t) {
    await boot(t);
    for (const [label, name, state, from] of CONV_STATES) {
      await open(t, name, from ?? 'rail-inbox');
      if (label === 'a-working-subagent') {
        const latest = t.page.getByTestId('conv-latest'); if (await latest.count()) { await latest.click().catch(() => {}); await t.settle(300); }
        await t.page.getByTestId('conv-scroller').evaluate((e) => (e.scrollTop = e.scrollHeight)); await t.settle(300);
        await t.shot(label + '-bottom', m('Conversation', state + ' (scrolled to the running turn)', 'rail-sessions; click row; scroll to bottom'));
        // expand the Edit of inbox.ts to show its diff
        const edit = t.page.getByText('/lib/inbox.ts', { exact: false }).last();
        if (await edit.count()) { await edit.click().catch(() => {}); await t.settle(900); await t.page.getByTestId('conv-scroller').evaluate((e) => (e.scrollTop = e.scrollHeight - e.clientHeight - 250)); await t.settle(300); }
        else log.noEditLine = true;
      }
      await t.shot(label, m('Conversation', state, `${from ?? 'rail-inbox'}; click row "${name}"`));
    }
  } });
}
// remaining states, dark only
runs.push({ name: 'conv-more', world: full, extraFile: EXTRA, async steps(t) {
  await boot(t);
  const more = [
    ['stuck-auth-menu', 'Tax rounding', 'stuck: auth menu'],
    ['stuck-trust-prompt', 'Orbit palette port', 'stuck: trust prompt'],
    ['stuck-reconnect', 'Offline queue', 'stuck: reconnecting'],
    ['stuck-oom', 'Hub e2e runner', 'stuck: out of memory'],
    ['done-ci-red', 'Widget crash fix', 'idle with failing CI on open PR #131'],
    ['jev-proposed', 'Dunning emails', 'Jev: probably waiting (question at end of turn)'],
    ['context-full', 'Federation e2e', 'working with context at 93%'],
    ['stale-working', 'Ledger export', 'stale working (demoted by tick)'],
    ['host-down', 'Load test', 'session on unreachable host oci-arm'],
    ['shared-answer', 'Pricing page copy', "Ada's session shared with me to answer, permission dialog"],
  ];
  for (const [label, name, state] of more) {
    await open(t, name);
    await t.shot(label, m('Conversation', state, `rail-inbox; click row "${name}"`));
  }
  for (const [label, name, state] of [['lost', 'SQLite bench', 'lost (host reboot)'], ['bg-agent', 'Explore inbox callers', 'background agent (kind bg)'], ['shared-watch', 'CI cache warmup', "Ada's session shared with me to watch"], ['idle', 'Docs pass', 'idle, seen']]) {
    await t.click('rail-sessions');
    const r = rowOf(t, name);
    if (!(await r.count())) { log['missing-row-' + label] = 'row not in Sessions list'; continue; }
    await open(t, name, null);
    await t.shot(label, m('Conversation', state, `rail-sessions; click row "${name}"`));
  }
} });

// ── 4. Answer UI up close + keys ──
runs.push({ name: 'answer', world: full, extraFile: EXTRA, async steps(t) {
  await boot(t);
  const card = () => t.page.locator('section.of-question').first();
  for (const [label, name] of [['perm', 'Stripe webhook retry'], ['choice', 'Push notification actions'], ['multi', 'Migration 208 cleanup'], ['freetext', 'Invoice PDF layout'], ['shared-perm', 'Pricing page copy']]) {
    await open(t, name);
    if (await card().count()) await t.shotOf(card(), `questioncard-${label}`, m('AnswerPrompt / QuestionCard', label, `rail-inbox; open "${name}"`));
    else log['no-card-' + label] = true;
  }
  // compact card in the sidebar row
  await t.click('rail-inbox');
  const rc = rowOf(t, 'Push notification actions');
  await t.shotOf(rc, 'row-compact-choice', m('Sidebar row', 'row of a session waiting on a choice', 'rail-inbox'));
  await t.shotOf(rowOf(t, 'Partial refunds'), 'row-limit-actions', m('Sidebar row', 'LimitActions on a row at the five-hour limit', 'rail-inbox'));
  // press keys on the permission dialog
  await open(t, 'Stripe webhook retry');
  await t.fake(() => { window.__fleetFake.calls.length = 0; });
  await t.page.locator('body').click({ position: { x: 1000, y: 300 } }).catch(() => {});
  await t.page.keyboard.press('1');
  await t.page.waitForTimeout(120);
  await t.shot('perm-key1-immediately', m('Conversation', 'right after pressing 1 on the permission dialog (~0.1 s)', 'open Stripe webhook retry; click transcript; press 1'));
  await t.page.waitForTimeout(500);
  await t.shot('perm-key1-0.6s', m('Conversation', '0.6 s after pressing 1', 'press 1; wait'));
  await t.page.waitForTimeout(2200);
  await t.shot('perm-key1-3s', m('Conversation', '3 s after pressing 1 (row went working)', 'press 1; wait'));
  log.permAnswerCalls = await t.fake(() => window.__fleetFake.calls.filter((c) => !/^plugin:|^pty_|session_presence|set_tray_state/.test(c.cmd)).map((c) => ({ cmd: c.cmd, args: c.args })));
  // choice: press 2
  await open(t, 'Push notification actions');
  await t.fake(() => { window.__fleetFake.calls.length = 0; });
  await t.page.locator('body').click({ position: { x: 1000, y: 300 } }).catch(() => {});
  await t.page.keyboard.press('2'); await t.page.waitForTimeout(150);
  await t.shot('choice-key2-immediately', m('Conversation', 'right after pressing 2 on a multiple-choice question', 'open Push notification actions; press 2'));
  await t.page.waitForTimeout(2600);
  await t.shot('choice-key2-3s', m('Conversation', '3 s after pressing 2', 'press 2; wait'));
  log.choiceAnswerCalls = await t.fake(() => window.__fleetFake.calls.filter((c) => !/^plugin:|^pty_|session_presence|set_tray_state/.test(c.cmd)).map((c) => ({ cmd: c.cmd, args: c.args })));
  // multi: press 2 then 3
  await open(t, 'Migration 208 cleanup');
  await t.fake(() => { window.__fleetFake.calls.length = 0; });
  await t.page.locator('body').click({ position: { x: 1000, y: 300 } }).catch(() => {});
  await t.page.keyboard.press('2'); await t.page.waitForTimeout(150);
  await t.shot('multi-key2-immediately', m('Conversation', 'multi-select right after pressing 2 (toggle)', 'open Migration 208 cleanup; press 2'));
  await t.page.waitForTimeout(1200);
  if (await card().count()) await t.shotOf(card(), 'multi-key2-card-1.3s', m('QuestionCard', 'multi-select 1.3 s after pressing 2', 'press 2; wait'));
  log.multiAnswerCalls = await t.fake(() => window.__fleetFake.calls.filter((c) => !/^plugin:|^pty_|session_presence|set_tray_state/.test(c.cmd)).map((c) => ({ cmd: c.cmd, args: c.args })));
  // free text: Answer in your own words
  await open(t, 'Invoice PDF layout');
  const own = t.page.getByText('Answer in your own words', { exact: false }).first();
  if (await own.count()) { await own.click(); await t.settle(900); await t.shot('freetext-own-words', m('Conversation', 'after clicking "Answer in your own words…"', 'open Invoice PDF layout; click Answer in your own words')); }
  // form card
  await open(t, 'Hub reconnect backoff');
  const form = t.page.locator('[data-testid^="form-card"], .form-card').first();
  if (await form.count()) await t.shotOf(form, 'form-card', m('FormCard', 'pending chat form', 'open Hub reconnect backoff'));
  else log.noFormCard = true;
  // MCP confirm dialog (a host-side MCP call asks for confirmation)
  await t.fake(() => window.__fleetFake.emit('mcp:confirm-required', { nonce: 'n-1', tool: 'kill_session', summary: 'host=mercury name=billing-webhook-retry', caller: 'host:htz-1', operator: false, asked_at: Math.floor(Date.now() / 1000) - 5 }));
  await t.settle(800);
  await t.shot('mcp-confirm-dialog', m('McpConfirmDialog', 'an MCP tool call from host htz-1 asks to kill a session', 'emit mcp:confirm-required'));
} });

// ── 3j ReplyActions + quick replies ──
runs.push({ name: 'reply', world: full, extraFile: EXTRA, async steps(t) {
  await boot(t);
  await open(t, 'Settings search', 'rail-sessions');
  const text = t.page.getByTestId('conv-text').last();
  await text.hover(); await t.settle(400);
  await t.shot('done-hover-reply', m('Conversation', 'finished turn, pointer over the reply (ReplyActions)', 'open Settings search; hover the last reply'));
  const footer = t.page.getByTestId('conv-reply-footer').last();
  if (await footer.count()) await t.shotOf(footer, 'reply-footer', m('ReplyActions', 'reply footer', 'hover reply'));
  const chips = t.page.getByTestId('conv-chips');
  if (await chips.count()) await t.shotOf(t.page.getByTestId('conv-composer'), 'composer-with-chips', m('Composer', 'quick replies + prompt suggestion', 'open Settings search'));
} });

// ── 5. Composer ──
runs.push({ name: 'composer', world: full, extraFile: EXTRA, async steps(t) {
  await boot(t);
  await open(t, 'Invoice PDF layout');
  const comp = () => t.page.getByTestId('conv-composer');
  await t.shotOf(comp(), 'empty', m('Composer', 'empty (session waiting on a question)', 'open Invoice PDF layout'));
  await open(t, 'Docs pass', 'rail-sessions');
  await t.shotOf(comp(), 'empty-idle', m('Composer', 'empty, idle session', 'rail-sessions; open Docs pass'));
  await t.page.getByTestId('conv-composer-input').fill('Proofread docs/hub.md again and fix the verdict links');
  await t.settle(300);
  await t.shotOf(comp(), 'typed', m('Composer', 'with typed text', 'type into the box'));
  await t.page.getByTestId('conv-composer-input').fill('/');
  await t.settle(400);
  await t.shot('slash-menu', m('Composer', 'slash menu after typing /', 'type /'));
  await t.page.getByTestId('conv-composer-input').fill('Proofread docs/hub.md again and fix the verdict links');
  await t.click('conv-send-later'); await t.settle(500);
  await t.shot('send-later', m('SendLaterSheet', 'open', 'type a prompt; click the clock button'));
  await t.key('Escape'); await t.settle(300);
  await t.click('conv-attach-button'); await t.settle(600);
  await t.shotOf(comp(), 'attached', m('Composer', 'after the attach picker returned two files', 'click attach (native picker answered by the fake)'));
  if (await t.page.getByTestId('conv-tune-button').count()) {
    await t.click('conv-tune-button'); await t.settle(400);
    await t.shot('tune-open', m('PromptTune', 'tune panel open', 'click Tune'));
    const run = t.page.getByTestId('conv-tune-run');
    if (await run.count()) { await run.click(); await t.settle(900); await t.shot('tune-result', m('PromptTune', 'rewrite proposed', 'Tune → Run')); }
    const close = t.page.getByTestId('conv-tune-close'); if (await close.count()) await close.click();
  } else log.noTune = true;
  await t.shot('send-later-disabled-with-files', m('Composer', 'Send later with files attached (button disabled)', 'attach files; look at clock button'));
} });

// ── 6. Inspector / Details ──
runs.push({ name: 'inspector', world: full, extraFile: EXTRA, async steps(t) {
  await boot(t);
  await open(t, 'Stripe webhook retry');
  await t.shot('needs-you-open', m('Inspector', 'open beside a permission-blocked session', 'open Stripe webhook retry (inspector on by default)'));
  await t.key('Control+Alt+b');
  await t.shot('needs-you-closed', m('Inspector', 'closed with Ctrl+Alt+B', 'press Ctrl+Alt+B'));
  await t.key('Control+Alt+b');
  await open(t, 'Inbox grouping', 'rail-sessions');
  await t.shot('working-open', m('Inspector', 'beside a working session', 'rail-sessions; open Inbox grouping'));
  await t.click('stab-details'); await t.settle(1200);
  await t.shot('details-working', m('SessionDetails', 'Details tab of a working session', 'click Details tab'));
  await t.page.mouse.wheel(0, 900); await t.settle(400);
  await t.shot('details-working-scrolled', m('SessionDetails', 'Details tab scrolled (tasks, devices)', 'Details; scroll'));
  const tasks = t.page.getByTestId('session-tasks');
  if (await tasks.count()) { await tasks.scrollIntoViewIfNeeded(); await t.shotOf(tasks, 'session-tasks', m('SessionTasks', 'three links: primary, suggested, past', 'Details; scroll to Tasks')); }
  const dev = t.page.getByTestId('session-devices');
  if (await dev.count()) await t.shotOf(dev, 'session-devices', m('SessionDevices', 'as rendered', 'Details'));
  else log.noDevices = 'session-devices not rendered';
  await open(t, 'Stripe webhook retry');
  await t.click('stab-details'); await t.settle(1200);
  await t.shot('details-needs-you', m('SessionDetails', 'Details tab of a permission-blocked session', 'open Stripe webhook retry; Details tab'));
} });

// ── 7. Terminal ──
const O = '\x1b[38;5;214m', R = '\x1b[0m';
const vis = (x) => x.replace(/\x1b\[[0-9;]*m/g, '');
const boxLine = (x) => `${O}│${R} ${x}${' '.repeat(Math.max(0, 76 - vis(x).length))} ${O}│${R}`;
const TUI = '\x1b[2J\x1b[H' + [
  '\x1b[38;5;174m✻\x1b[0m Welcome back! \x1b[2m/home/martin/src/billing-api\x1b[0m',
  '',
  '\x1b[1m⏺\x1b[0m pytest tests/webhooks passes: \x1b[1m23 passed in 4.1s\x1b[0m. Committing and pushing.',
  '',
  '\x1b[32m⏺\x1b[0m \x1b[1mBash\x1b[0m(git commit -am "BILL-88: retry failed Stripe webhooks")',
  '  ⎿  [BILL-88-stripe-webhook-retry 4c1e9a2] BILL-88: retry failed Stripe webhooks',
  '      3 files changed, 112 insertions(+), 9 deletions(-)',
  '',
  `${O}╭${'─'.repeat(78)}╮${R}`,
  boxLine('\x1b[1mBash command\x1b[0m'),
  boxLine(''),
  boxLine('  git push origin main'),
  boxLine('  \x1b[2mPush the BILL-88 commit to origin\x1b[0m'),
  boxLine(''),
  boxLine('Do you want to proceed?'),
  boxLine('\x1b[36m❯ 1. Yes\x1b[0m'),
  boxLine("  2. Yes, and don't ask again for git push commands in"),
  boxLine('     /home/martin/src/billing-api'),
  boxLine('  3. No, and tell Claude what to do differently \x1b[2m(esc)\x1b[0m'),
  `${O}╰${'─'.repeat(78)}╯${R}`,
].join('\r\n');
runs.push({ name: 'terminal', world: full, extraFile: EXTRA, async steps(t) {
  await boot(t);
  await t.fake((s) => { window.__D1A.ptyQueue = [s]; }, TUI);
  await open(t, 'Stripe webhook retry');
  await t.click('stab-agent'); await t.settle(2500);
  await t.shot('claude-code-permission', m('Terminal (Claude Code tab)', 'pane bytes of a Claude Code permission prompt fed through pty_drain', 'open Stripe webhook retry; Claude Code tab'));
  await t.click('stab-terminals'); await t.settle(1500);
  await t.shot('terminals-tab', m('Terminals tab', 'no shell terminals', 'click Terminals tab'));
  await t.click('stab-agent'); await t.settle(800);
  const sk = t.page.getByRole('button', { name: /keys/i }).first();
  if (await sk.count()) { await sk.click().catch(() => {}); await t.settle(500); if (await t.page.getByTestId('send-keys').count()) await t.shot('send-keys', m('SendKeys', 'open', 'Claude Code tab; Keys button')); else log.noSendKeys = true; }
  log.terminalCalls = await t.fake(() => [...new Set(window.__fleetFake.calls.map((c) => c.cmd).filter((c) => c.startsWith('pty')))]);
} });

// ── 8. Attention outside the list ──
runs.push({ name: 'attention', world: full, extraFile: EXTRA, async steps(t) {
  await boot(t);
  await open(t, 'Settings search', 'rail-sessions');
  await t.shot('before', m('Attention', 'Settings search open, before any change', 'rail-sessions; open Settings search'));
  await t.fake(() => window.__setRow(1, { claude_status: 'blocked', current_activity: 'Waiting for permission: Bash(rm -rf node_modules)', pending_input: { kind: 'permission', question: 'Do you want to proceed?', detail: 'Bash(rm -rf node_modules && pnpm install)', options: [{ n: 1, label: 'Yes', selected: true }, { n: 2, label: "Yes, and don't ask again for rm commands in /Users/martin/dev/claude-fleet", selected: false }, { n: 3, label: 'No, and tell Claude what to do differently (esc)', selected: false }] }, last_activity_at: Math.floor(Date.now() / 1000) }));
  await t.page.waitForTimeout(400);
  await t.shot('after-blocked-0.4s', m('Attention', '0.4 s after "Inbox grouping" went blocked (session:updated)', 'emit session:updated for session 1 → blocked'));
  await t.page.waitForTimeout(2000);
  await t.shot('after-blocked-2.4s', m('Attention', '2.4 s after the change', 'wait'));
  // a stuck transition (Attention.svelte announces stuck)
  await t.fake(() => window.__setRow(21, { claude_status: 'idle', stuck_kind: 'press_enter', stuck_since: Math.floor(Date.now() / 1000) }));
  await t.page.waitForTimeout(800);
  await t.shot('after-stuck', m('Attention', 'after "Federation e2e" became stuck (press Enter)', 'emit session:updated for session 21 → stuck_kind press_enter'));
  const toasts = t.page.getByTestId('toasts');
  if (await toasts.count() && await toasts.isVisible()) await t.shotOf(toasts, 'toasts', m('Toasts', 'toasts on screen', 'after the two changes'));
  await t.shotOf(t.page.locator('footer, [data-testid="status-bar"], .of-statusbar').last(), 'status-bar', m('StatusBar', 'after the changes', 'after the two changes')).catch(() => (log.noStatusBar = true));
  await t.shotOf(t.page.getByTestId('rail-inbox'), 'rail-badge', m('Rail', 'Inbox rail badge after the changes', 'after the two changes'));
  const dl = t.page.getByText('Downloads…').first();
  if (await dl.count()) {
    await dl.click(); await t.settle(700);
    const tab = t.page.getByText(/^Notifications/).first();
    if (await tab.count()) { await tab.click(); await t.settle(500); }
    await t.shot('notifications', m('NotificationList', 'Notifications tab of the Downloads sheet after the changes', 'click "Downloads…" in the status bar; Notifications tab'));
    await t.key('Escape'); await t.settle(300);
  }
  await t.click('rail-inbox'); await park(t);
  await t.shot('inbox-before-third', m('Attention', 'Inbox open, before a third session goes blocked', 'rail-inbox'));
  await t.fake(() => window.__setRow(16, { claude_status: 'blocked', idle_since: null, last_activity_at: Math.floor(Date.now() / 1000), pending_input: { kind: 'input', question: 'Should I also update docs/architecture.md?', options: [{ n: 1, label: 'Yes', selected: true }, { n: 2, label: 'No', selected: false }, { n: 3, label: 'Type something.', selected: false }] } }));
  await t.page.waitForTimeout(1200);
  await t.shot('inbox-after-third', m('Attention', 'Inbox open, 1.2 s after "Docs pass" went blocked', 'emit session:updated for session 16 → blocked'));
  log.trayCalls = await t.fake(() => window.__fleetFake.calls.filter((c) => /tray|notif/.test(c.cmd)).map((c) => ({ cmd: c.cmd, args: c.args })).slice(-6));
} });

// ── 1. Sessions list ──
for (const [theme, vp] of [['dark', null], ['light', null], ['dark', W1100]]) {
  runs.push({ name: 'sessions', world: full, theme, viewport: vp ?? undefined, extraFile: EXTRA, async steps(t) {
    await boot(t);
    await t.click('rail-sessions'); await park(t);
    await t.shot('default', m('Sessions', 'default (Group: Project)', 'rail-sessions'));
    if (theme !== 'dark' || vp) return;
    for (const g of ['work', 'state', 'host', 'agent', 'org']) {
      await groupBy(t, 'group-select', g); await park(t);
      await t.shot('group-' + g, m('Sessions', 'Group: ' + g, 'rail-sessions; Group select → ' + g));
    }
    await groupBy(t, 'group-select', 'project');
    await t.click('filters-open');
    await t.shot('filters-open', m('Sessions', 'Filters panel open', 'click Filters'));
    await t.key('Escape');
    if (await t.page.getByTestId('filter-panel').isVisible().catch(() => false)) await t.click('filters-open');
    const r = rowOf(t, 'Stripe webhook retry');
    await r.hover(); await t.settle(300);
    await t.shot('row-hover', m('Sessions', 'row hovered', 'hover "Stripe webhook retry"'));
    await r.click(); await t.settle(500);
    await t.shot('row-centre-click', m('Sessions', 'what a click in the middle of a hovered row does', 'hover row; click its centre (Playwright default click point)'));
    await t.key('Escape');
    await r.getByTestId('row-menu-open').click(); await t.settle(400);
    await t.shot('row-menu', m('Sessions', 'row menu open', 'hover row; click ⋯'));
    await t.key('Escape'); await park(t);
    await t.click('view-options-open');
    await t.shot('view-options', m('Sessions', 'view options (⋯) menu', 'click ⋯ beside Group'));
    await t.click('select-mode'); await t.settle(300);
    for (const n of ['Stripe webhook retry', 'Invoice PDF layout', 'Tax rounding']) await rowOf(t, n).getByTestId('select-box').click().catch(() => (log['nobox-' + n] = true));
    await park(t);
    await t.shot('select-bulk', m('Sessions', 'select mode, 3 rows selected, bulk bar', 'view options → Select; tick 3 rows'));
    await t.key('Escape');
    const clear = t.page.getByTestId('bulk-clear'); if (await clear.count()) await clear.click();
    for (const tab of ['mine', 'shared']) {
      const el = t.page.locator(`[data-testid^="scope-tab-${tab}"]`).first();
      if (await el.count()) { await el.click(); await t.settle(400); await park(t); await t.shot('tab-' + tab, m('Sessions', 'scope tab ' + tab, 'click tab ' + tab)); }
      else log['noTab-' + tab] = await t.fake(() => [...document.querySelectorAll('[data-testid^="scope-tab"]')].map((e) => e.dataset.testid));
    }
    const all = t.page.locator('[data-testid^="scope-tab-all"]').first(); if (await all.count()) await all.click();
    await t.page.getByTestId('sidebar-search').locator('input').or(t.page.getByTestId('sidebar-search')).first().fill('billing');
    await t.settle(500); await park(t);
    await t.shot('search-billing', m('Sessions', 'search "billing"', 'type billing in the search box'));
  } });
}
runs.push({ name: 'sessions-comfortable', world: full, prefs: { 'ui.rowDensity': 'comfortable' }, extraFile: EXTRA, async steps(t) {
  await boot(t); await t.click('rail-sessions'); await park(t);
  await t.shot('default', m('Sessions', 'row density comfortable', 'pref ui.rowDensity=comfortable; rail-sessions'));
  await t.click('rail-inbox'); await park(t);
  await t.shot('inbox', m('Inbox', 'row density comfortable', 'pref ui.rowDensity=comfortable; rail-inbox'));
} });

const wanted = only ? only.split(',') : null;
const picked = wanted ? runs.filter((r) => wanted.some((w) => r.name === w)) : runs;
const report = await capture({ outDir: D, cacheDir: D + '/vite-cache', runs: picked });
writeFileSync(D + '/capture-log.json', JSON.stringify({ log, report: report.map((e) => ({ run: e.run, theme: e.theme, w: e.viewport.width, errors: e.errors, unknown: e.unknown })) }, null, 2));
for (const e of report) console.log(e.run, e.theme, e.viewport.width, 'shots', e.shots.length, 'errors', e.errors.length, e.errors.slice(0, 3));
