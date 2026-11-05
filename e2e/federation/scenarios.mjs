// Federation e2e scenarios (docs/federation-e2e.md).
//
// Each scenario is a world (the fake hubs, fake-hubs.js) and the steps an
// agent would take in the window: click, look, screenshot, check. Checks
// assert what a person would see, by data-testid; `t.calls()` shows which
// hub each call went to.
//
// World shape:
//   primary: { url?, label?, color?, hosts: [alias | {alias, ...HostRow}],
//              sessions: [{host, name, ...SessionRow}] }   no url = standalone
//   extras:  [{ id, label, url?, color?, hosts, sessions,
//              connection?: HubConnection | null, unavailable?, duplicate_of? }]
//   retryBringsBack?: [fleet id]   fleet_retry_now reconnects these

const PAPAYA = {
  id: 'papaya',
  label: 'Papaya POS',
  url: 'https://fleet-hub.papayapos.dev',
  color: '#c2410c',
  hosts: ['pos-build-1', 'pos-build-2', { alias: 'pos-gpu', reachable: false }],
  sessions: [
    { host: 'pos-build-1', name: 'pos-api', claude_status: 'working' },
    { host: 'pos-build-2', name: 'pos-web', claude_status: 'blocked' },
    { host: 'pos-build-1', name: 'pos-payments' },
  ],
};

const ACME = {
  id: 'acme',
  label: 'Acme test',
  url: 'https://fleet-hub.tst.example.dev',
  color: '#7c3aed',
  hosts: ['test-host-1', 'test-host-2'],
  sessions: [{ host: 'test-host-1', name: 'acme-demo' }],
};

const OWN_HUB = {
  url: 'https://fleet.rlt.sk',
  label: 'Own hub',
  color: '#2563eb',
  hosts: ['mac', 'mefistos', 'claude-fleet-htz', { alias: 'claude-fleet-oci', reachable: false }],
  sessions: [
    { host: 'mefistos', name: 'dev-mef', claude_status: 'working' },
    { host: 'mac', name: 'fleet-ui' },
  ],
};

const STANDALONE = {
  hosts: ['local', 'mefistos'],
  sessions: [{ host: 'local', name: 'dev-local' }, { host: 'mefistos', name: 'dev-mef' }],
};

/** Open Accounts › All hosts (the Hosts page), on its table: the page
 *  reopens on the host it last showed, so step back to the list. */
async function openHosts(t) {
  for (let i = 0; i < 3 && !(await t.byTestId('hosts-view').isVisible()); i++) {
    await t.page.keyboard.press('Control+Shift+H');
    await t.settle(300);
  }
  await t.byTestId('hosts-view').waitFor({ state: 'visible', timeout: 5000 });
  const back = t.byTestId('hosts-back-to-table');
  if (await back.isVisible()) await back.click();
  await t.settle(400);
}

/** Set the sidebar's fleet filter the way a person does: on the Sessions
 *  sidebar (the Hosts page has no switcher of its own), then come back. */
async function pickFleet(t, id) {
  const onHosts = await t.byTestId('hosts-view').isVisible();
  if (onHosts) await t.byTestId('rail-sessions').click();
  await t.byTestId(`fleet-filter-${id}`).click();
  await t.settle(300);
  if (onHosts) await openHosts(t);
}

export const scenarios = [
  {
    name: 'own-hub-plus-papaya',
    title: 'Own hub + one federated fleet: both on the sidebar and on All hosts',
    world: { primary: OWN_HUB, extras: [PAPAYA] },
    async run(t) {
      await t.visible('fleet-switcher', 'the fleet switcher shows with two fleets');
      await t.visible('fleet-filter-primary');
      await t.visible('fleet-filter-papaya');
      await t.text('fleet-count-papaya', /^3$/, 'Papaya POS group counts its 3 sessions');
      await t.shot('sidebar-all-fleets');
      await openHosts(t);
      await t.visible('hosts-own-fleet', "own hub's hosts are on All hosts");
      await t.count('[data-testid="hosts-own-fleet"] [data-testid="hosts-table-row"]', 4, 'all 4 own hosts are listed');
      await t.visible('fleet-hosts-papaya', "Papaya POS's hosts are on All hosts");
      await t.count('[data-testid^="fleet-host-papaya-"]', 3, 'all 3 Papaya hosts are listed');
      await t.shot('all-hosts');
      const calls = await t.calls();
      t.check(
        calls.some((c) => c.cmd === 'list_hosts' && c.fleet === 'papaya') && calls.some((c) => c.cmd === 'list_hosts' && c.fleet === null),
        'hosts were listed from both hubs',
      );
    },
  },
  {
    name: 'standalone-plus-papaya',
    title: 'This machine (no hub) + a federated fleet',
    world: { primary: STANDALONE, extras: [PAPAYA] },
    async run(t) {
      await t.visible('fleet-filter-papaya');
      await t.text('fleet-state-primary', /this machine/, 'own fleet reads "this machine"');
      await t.shot('sidebar');
      await openHosts(t);
      await t.visible('hosts-own-fleet', 'own hosts are on All hosts');
      await t.visible('fleet-hosts-papaya');
      await t.shot('all-hosts');
    },
  },
  {
    name: 'filter-one-fleet',
    title: 'Fleet filter on one fleet: the other fleet is hidden everywhere, and says so',
    world: { primary: OWN_HUB, extras: [PAPAYA] },
    async run(t) {
      await pickFleet(t, 'papaya');
      await t.visible('fleet-group-papaya', 'Papaya group stays');
      await t.shot('sidebar-papaya-only');
      await openHosts(t);
      await t.visible('fleet-hosts-papaya');
      await t.visible('hosts-primary-filtered', 'All hosts says own hosts are hidden by the filter');
      await t.shot('hosts-papaya-only');
      await pickFleet(t, 'primary');
      await t.visible('hosts-own-fleet', 'own hosts come back with the filter on own hub');
      await t.hidden('fleet-hosts-papaya', 'Papaya hosts hide with the filter on own hub');
      await t.shot('hosts-own-only');
      await pickFleet(t, 'all');
      await t.visible('hosts-own-fleet');
      await t.visible('fleet-hosts-papaya');
      await t.shot('hosts-all-again');
    },
  },
  {
    name: 'remembered-filter',
    title: 'A fleet filter remembered from last time is visible on launch',
    world: { primary: OWN_HUB, extras: [PAPAYA] },
    prefs: { 'ui.fleetFilter': 'papaya' },
    async run(t) {
      const checked = await t.byTestId('fleet-filter-papaya').getAttribute('aria-checked');
      t.check(checked === 'true', 'the Papaya chip is the checked one at launch', `aria-checked=${checked}`);
      await openHosts(t);
      await t.visible('hosts-primary-filtered', 'All hosts explains why own hosts are missing');
      await t.shot('hosts-launch-filtered');
    },
  },
  {
    name: 'three-hubs',
    title: 'Three hubs: each chip shows its own fleet only',
    world: { primary: OWN_HUB, extras: [PAPAYA, ACME] },
    async run(t) {
      await t.visible('fleet-filter-acme');
      await t.shot('sidebar-three-fleets');
      for (const id of ['primary', 'papaya', 'acme']) {
        await pickFleet(t, id);
        await openHosts(t);
        for (const other of ['papaya', 'acme']) {
          if (other === id) await t.visible(`fleet-hosts-${other}`, `filter ${id}: ${other} hosts shown`);
          else await t.hidden(`fleet-hosts-${other}`, `filter ${id}: ${other} hosts hidden`);
        }
        if (id === 'primary') await t.visible('hosts-own-fleet', 'filter primary: own hosts shown');
        else await t.visible('hosts-primary-filtered', `filter ${id}: own hosts hidden, with a note`);
        await t.shot(`hosts-filter-${id}`);
      }
      await pickFleet(t, 'all');
      await t.visible('hosts-own-fleet');
      await t.visible('fleet-hosts-papaya');
      await t.visible('fleet-hosts-acme');
      await t.shot('hosts-all-three');
    },
  },
  {
    name: 'offline-federated',
    title: 'A federated hub that is offline, then comes back',
    world: {
      primary: OWN_HUB,
      extras: [{ ...PAPAYA, connection: { state: 'offline', attempt: 3, retry_in_secs: 30, reason: 'connection refused' } }],
      retryBringsBack: ['papaya'],
    },
    async run(t) {
      await t.text('fleet-state-papaya', /offline/, 'Papaya reads offline');
      await t.visible('fleet-trouble-papaya', 'its group says what is wrong');
      await t.count('[data-testid^="fleet-session-papaya-"]', 0, 'no Papaya sessions while it is offline');
      await t.hidden('fleet-empty-papaya', 'it does not claim "No sessions" — it was never listed');
      await t.shot('sidebar-offline');
      await openHosts(t);
      await t.visible('hosts-own-fleet', 'own hosts are unaffected');
      await t.shot('hosts-offline');
      await t.fake(() => window.__fleetFake.setFleet('papaya', { connection: { state: 'connected' } }));
      await t.settle(800);
      await t.count('[data-testid^="fleet-host-papaya-"]', 3, 'Papaya hosts appear once it reconnects');
      await t.shot('hosts-reconnected');
    },
  },
  {
    name: 'unavailable-federated',
    title: 'A configured fleet this launch cannot use (no stored token)',
    world: { primary: OWN_HUB, extras: [{ ...PAPAYA, connection: null, unavailable: 'no stored token for this fleet' }] },
    async run(t) {
      await t.text('fleet-state-papaya', /unavailable/);
      await t.visible('fleet-unavailable-papaya', 'its group gives the reason');
      const calls = await t.calls();
      t.check(!calls.some((c) => c.fleet === 'papaya' && c.cmd.startsWith('list_')), 'nothing is listed from an unavailable fleet');
      await t.shot('sidebar-unavailable');
    },
  },
  {
    name: 'duplicate-hub',
    title: 'The same hub paired twice: its rows are not shown twice',
    world: { primary: OWN_HUB, extras: [{ ...PAPAYA, id: 'again', label: 'Own hub again', duplicate_of: 'primary' }] },
    async run(t) {
      await t.visible('fleet-duplicate-again', 'the duplicate says it is the same hub');
      await t.count('[data-testid^="fleet-session-again-"]', 0, 'none of its sessions are shown');
      await t.shot('sidebar-duplicate');
    },
  },
  {
    name: 'same-ids-two-hubs',
    title: 'Session 1 on host "mac" exists on both hubs: each opens from its own hub',
    world: {
      primary: { ...OWN_HUB, hosts: ['mac'], sessions: [{ host: 'mac', name: 'own-on-mac' }] },
      extras: [{ ...PAPAYA, hosts: ['mac'], sessions: [{ host: 'mac', name: 'papaya-on-mac' }] }],
    },
    async run(t) {
      await t.visible('fleet-open-papaya-1');
      await t.shot('sidebar-colliding-ids');
      await t.byTestId('fleet-open-papaya-1').click();
      await t.settle(800);
      await t.visible('fleet-session-overlay', "Papaya's session opens in the main pane");
      await t.onTop('fleet-session-view', "Papaya's session is on top, not covered by the empty state");
      await t.shot('papaya-session-open');
      const calls = await t.calls();
      const routed = calls.filter((c) => ['session_conversation', 'session_conversations', 'capture_session'].includes(c.cmd));
      t.check(routed.length > 0, 'opening it read the conversation', 'no session_conversation/capture_session call');
      t.check(routed.every((c) => c.fleet === 'papaya'), "every read for Papaya's session went to Papaya", JSON.stringify(routed));
    },
  },
  {
    name: 'settings-fleets',
    title: 'Settings › Hub & sync lists each fleet with its state',
    world: {
      primary: OWN_HUB,
      extras: [PAPAYA, { ...ACME, connection: { state: 'reconnecting', attempt: 1, retry_in_secs: 5, reason: 'timed out' } }],
    },
    async run(t) {
      await t.page.keyboard.press('Control+,');
      await t.settle(500);
      const section = t.byTestId('fleets-section');
      if (!(await section.isVisible())) {
        await t.page.getByRole('button', { name: /Hub/ }).first().click().catch(() => {});
        await t.settle(400);
      }
      await section.scrollIntoViewIfNeeded().catch(() => {});
      await t.visible('fleets-row-papaya');
      await t.visible('fleets-row-acme');
      await t.shot('settings-fleets');
    },
  },
];
