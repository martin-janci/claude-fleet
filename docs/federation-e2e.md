# Federation e2e: the real window against fake hubs

`scripts/federation-e2e.mjs` runs the desktop frontend in Chromium against a
set of fake hubs, drives it through multi-hub scenarios, saves a screenshot at
every step and checks what a person would see. It needs no Rust build, no
Tauri libraries and no real hub, so an agent can run it in a cloud container
and look at the screenshots itself.

```bash
pnpm install --frozen-lockfile
node scripts/federation-e2e.mjs                  # every scenario (~1 min)
node scripts/federation-e2e.mjs --only offline   # scenarios whose name contains "offline"
node scripts/federation-e2e.mjs --list
node scripts/federation-e2e.mjs --out /tmp/shots --headed
```

Output goes to `target/federation-e2e/`: one PNG per step
(`<scenario>--NN-<step>.png`), `report.json` (checks, screenshots, the
commands nothing answered, console errors) and `index.html`, a contact sheet
of every screenshot with its checks. Exit status 1 when any check failed.
`--out <folder>` writes there instead; the folder is created when missing,
and a run replaces only its own files in it (`*--NN-*.png`, `report.json`,
`index.html`), never anything else.

Playwright is not a project dependency. Cloud containers have it
preinstalled; elsewhere run `npm i -g playwright && npx playwright install chromium` once.

## How it works

| Piece | File |
|---|---|
| Runner: starts Vite, one browser context per scenario, report | `scripts/federation-e2e.mjs` |
| The app's Vite config without the Reticle dev plugin | `e2e/federation/vite.config.mjs` |
| Fake backend: `window.__TAURI_INTERNALS__` + event plugin, the fake hubs | `e2e/federation/fake-hubs.js` |
| The scenarios | `e2e/federation/scenarios.mjs` |

The fake stands in for the Rust backend at the IPC boundary. Every `invoke`
is answered from the scenario's world, and routed the way
`backend/fleets.rs` routes it: no `fleet` argument goes to the primary hub,
`fleet: "<id>"` goes to that extra hub, and a hub that is offline,
reconnecting or unavailable answers `E_HUB_UNREACHABLE`. Each hub numbers its
sessions from 1 and can reuse host aliases, so ids collide across fleets the
way real ones do.

The runner reads `FLEET_SCOPED` from `src/lib/fleets.ts` and the fake records
any call that carries `fleet` for a command outside it. A scenario with such a
call fails: the real backend would ignore the argument and act on the
primary hub instead.

What it does not cover: the Rust side of federation (pairing, the hub
websocket, `Fleets::select`, the hub's own refusals). `scripts/hub-e2e.sh`
covers the real hub and agent; this harness covers what the window shows.

## Scenarios

| Name | What it checks |
|---|---|
| `own-hub-plus-papaya` | Own hub + Papaya POS: both fleets on the sidebar, and all hosts of both on All hosts |
| `standalone-plus-papaya` | This machine (no hub) + a federated fleet |
| `filter-one-fleet` | Fleet filter on Papaya, then on own hub, then All: each page hides the other fleet and says so |
| `remembered-filter` | A filter remembered from last time (Martin's bug, 2026-11-05): the note on All hosts names the fleet and Show all fleets brings own hosts back |
| `three-hubs` | Three hubs, each chip in turn on All hosts |
| `offline-federated` | A hub offline since launch: state, reason, no false "No sessions"; then it reconnects and its hosts appear |
| `unavailable-federated` | A configured fleet without a stored token: the reason is shown and nothing is listed from it |
| `duplicate-hub` | The same hub paired twice: the duplicate says so and its rows are not shown twice |
| `same-ids-two-hubs` | Session 1 on host `mac` on both hubs: Papaya's opens on top of the window, and every read goes to Papaya |
| `settings-fleets` | Settings › Hub & sync lists each fleet with its state |

## Writing a scenario

A scenario is `{ name, title, world, prefs?, viewport?, run(t) }` in
`scenarios.mjs`. The world:

```js
{
  primary: { url: 'https://fleet.rlt.sk', label: 'Own hub', hosts: ['mac', { alias: 'oci', reachable: false }],
             sessions: [{ host: 'mac', name: 'fleet-ui', claude_status: 'working' }] },   // no url = standalone
  extras: [{ id: 'papaya', label: 'Papaya POS', hosts: [...], sessions: [...],
             connection: { state: 'offline', attempt: 1, retry_in_secs: 30, reason: 'refused' },  // default connected
             unavailable: null, duplicate_of: null }],
  retryBringsBack: ['papaya'],   // Retry reconnects it
}
```

Hosts and sessions take any `HostRow` / `SessionRow` field over the defaults
in `fake-hubs.js`. `prefs` seeds `cf:pref:*` (for example
`{ 'ui.fleetFilter': 'papaya' }`); the first-run tour and coach marks are off.

`t` gives: `shot(label)`; `visible(testid)`, `hidden(testid)`,
`text(testid, /re/)`, `count(selector, n)` and `onTop(testid)` (nothing is
drawn over its centre: `visible` passes for a covered pane); `check(ok,
what, detail)`; `calls()` (every invoke with its `fleet`); `fake(fn)` to
change the world while the page runs, for example
`t.fake(() => window.__fleetFake.setFleet('papaya', { connection: { state: 'connected' } }))`;
and `page`, the Playwright page. A failed check does not stop the scenario,
so the screenshots after it still show what the window did.

A command nothing in the fake answers gets `[]` for a `list_*` and `null`
otherwise; `report.json` lists them per scenario under `unknown`. When a page
needs a real answer, add it to `STATIC` (same for every hub) or `PER_HUB` in
`fake-hubs.js`.

## Found with it

- An extra fleet's session opened UNDER the "No session open" empty state
  whenever this window had no session of its own selected (`App.svelte`
  `detailsMain`), so clicking it seemed to do nothing.
- A fleet offline since launch said "No sessions on Papaya POS." although
  it had never been listed.
