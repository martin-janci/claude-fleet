#!/usr/bin/env node
// Federation e2e: the real desktop frontend in Chromium against fake hubs.
//
//   node scripts/federation-e2e.mjs                 # every scenario
//   node scripts/federation-e2e.mjs --only offline  # names containing "offline"
//   node scripts/federation-e2e.mjs --list
//   node scripts/federation-e2e.mjs --out /tmp/shots --headed
//
// It starts Vite on the app (e2e/federation/vite.config.mjs), opens one
// fresh browser context per scenario with the fake backend injected
// (e2e/federation/fake-hubs.js), runs the scenario's steps, saves a
// screenshot at every `shot()`, and writes report.json + index.html (a
// contact sheet of every screenshot with its checks) to the output folder.
// Exit status 1 when any check failed. See docs/federation-e2e.md.
import { createRequire } from 'node:module';
import { execSync } from 'node:child_process';
import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const HARNESS = join(ROOT, 'e2e/federation');

// ── arguments ────────────────────────────────────────────────────────────
const argv = process.argv.slice(2);
const flag = (name) => argv.includes(name);
const value = (name, fallback) => {
  const i = argv.indexOf(name);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : fallback;
};
if (flag('--help') || flag('-h')) {
  console.log(readFileSync(fileURLToPath(import.meta.url), 'utf8').split('\n').slice(1, 14).join('\n').replace(/^\/\/ ?/gm, ''));
  process.exit(0);
}
const only = value('--only', null);
const outDir = resolve(value('--out', join(ROOT, 'target/federation-e2e')));
const headed = flag('--headed');

// ── playwright: the project's own, else a global install ────────────────
function loadPlaywright() {
  const req = createRequire(join(ROOT, 'package.json'));
  const tries = [() => req('playwright')];
  try {
    const g = execSync('npm root -g', { encoding: 'utf8' }).trim();
    tries.push(() => req(req.resolve('playwright', { paths: [g] })));
  } catch {
    /* no npm */
  }
  tries.push(() => req(req.resolve('playwright', { paths: ['/opt/node-tools/node_modules'] })));
  for (const t of tries) {
    try {
      return t();
    } catch {
      /* next */
    }
  }
  console.error(
    'playwright is not installed. Install it once (it is not a project dependency):\n' +
      '  npm i -g playwright && npx playwright install chromium',
  );
  process.exit(2);
}
const { chromium } = loadPlaywright();

// ── the scenarios ────────────────────────────────────────────────────────
const { scenarios } = await import(pathToFileURL(join(HARNESS, 'scenarios.mjs')).href);
const picked = scenarios.filter((s) => !only || s.name.includes(only));
if (flag('--list')) {
  for (const s of scenarios) console.log(`${s.name.padEnd(28)} ${s.title}`);
  process.exit(0);
}
if (!picked.length) {
  console.error(`no scenario matches "${only}" (--list shows them)`);
  process.exit(2);
}

// The commands the frontend may send to another fleet, read from the
// frontend's own list: the fake refuses any other command that carries a
// `fleet`, as the backend's `Fleets::select` does, and the run fails on it.
const fleetsTs = readFileSync(join(ROOT, 'src/lib/fleets.ts'), 'utf8');
const scopedBlock = fleetsTs.match(/export const FLEET_SCOPED = \[([\s\S]*?)\] as const/);
if (!scopedBlock) throw new Error('FLEET_SCOPED not found in src/lib/fleets.ts');
const FLEET_SCOPED = [...scopedBlock[1].matchAll(/'([a-z_]+)'/g)].map((m) => m[1]);

// ── vite ─────────────────────────────────────────────────────────────────
const { createServer } = await import(pathToFileURL(join(ROOT, 'node_modules/vite/dist/node/index.js')).href);
const server = await createServer({ configFile: join(HARNESS, 'vite.config.mjs'), server: { port: 0 } });
await server.listen();
const base = server.resolvedUrls.local[0];

// The output folder is whatever `--out` names (`.`, a Desktop): never remove
// it, only the files an earlier run wrote there.
mkdirSync(outDir, { recursive: true });
for (const f of readdirSync(outDir)) {
  if (/--\d\d-.*\.png$|^report\.json$|^index\.html$/.test(f)) rmSync(join(outDir, f), { force: true });
}
const shim = readFileSync(join(HARNESS, 'fake-hubs.js'), 'utf8');
const browser = await chromium.launch({ headless: !headed });

// Prefs every scenario starts from: no first-run tour, no coach marks.
const BASE_PREFS = { 'onboarding-welcomed': true, 'onboarding-dismissed': true, 'hints-enabled': false, 'tour-seen': true };

const report = [];
let failed = 0;
for (const sc of picked) {
  const entry = { name: sc.name, title: sc.title, checks: [], shots: [], errors: [], unknown: [] };
  report.push(entry);
  const context = await browser.newContext({ viewport: sc.viewport ?? { width: 1440, height: 900 }, deviceScaleFactor: 2, colorScheme: sc.colorScheme ?? 'light' });
  const prefs = { ...BASE_PREFS, ...(sc.prefs ?? {}) };
  await context.addInitScript({
    content:
      `window.__FLEET_WORLD__ = ${JSON.stringify(sc.world)};\n` +
      `window.__FLEET_SCOPED__ = ${JSON.stringify(FLEET_SCOPED)};\n` +
      `try { for (const [k, v] of Object.entries(${JSON.stringify(prefs)})) localStorage.setItem('cf:pref:' + k, JSON.stringify(v)); } catch {}`,
  });
  await context.addInitScript({ content: shim });
  const page = await context.newPage();
  page.on('pageerror', (e) => entry.errors.push(`pageerror: ${e.message}`));
  page.on('console', (m) => {
    if (m.type() === 'error') entry.errors.push(`console: ${m.text().slice(0, 400)}`);
  });

  let n = 0;
  const t = {
    page,
    byTestId: (id) => page.getByTestId(id),
    /** Save a screenshot named after the step. */
    async shot(label) {
      await page.waitForTimeout(250);
      const file = `${sc.name}--${String(++n).padStart(2, '0')}-${label}.png`;
      await page.screenshot({ path: join(outDir, file) });
      entry.shots.push({ file, label });
    },
    /** Record a check; a failed one fails the scenario but the steps go on,
     *  so the screenshots after it still show what the window did. */
    check(ok, what, detail) {
      entry.checks.push({ ok: !!ok, what, detail: ok ? undefined : detail });
      console.log(`  ${ok ? '✓' : '✗'} ${what}${ok || !detail ? '' : ` — ${detail}`}`);
    },
    async visible(testid, what) {
      const l = page.getByTestId(testid).first();
      const ok = await l.waitFor({ state: 'visible', timeout: 4000 }).then(() => true, () => false);
      t.check(ok, what ?? `${testid} is shown`, `no visible [data-testid=${testid}]`);
      return ok;
    },
    async hidden(testid, what) {
      const l = page.getByTestId(testid);
      const ok = await l.first().waitFor({ state: 'hidden', timeout: 4000 }).then(() => true, () => false);
      t.check(ok, what ?? `${testid} is not shown`, `[data-testid=${testid}] is visible`);
      return ok;
    },
    /** The element is what a person sees at its own centre: nothing else
     *  (an overlay, an empty state) is drawn over it. `visible` alone passes
     *  for a pane that is covered. */
    async onTop(testid, what) {
      const l = page.getByTestId(testid).first();
      await l.waitFor({ state: 'visible', timeout: 4000 }).catch(() => {});
      const res = await l
        .evaluate((el) => {
          const r = el.getBoundingClientRect();
          const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
          if (hit && el.contains(hit)) return null;
          for (let n = hit; n; n = n.parentElement) if (n.dataset?.testid) return n.dataset.testid;
          return hit ? hit.tagName : 'nothing';
        })
        .catch((e) => `not found (${e.message.split('\n')[0]})`);
      t.check(res === null, what ?? `${testid} is on top`, `covered by [data-testid=${res}]`);
      return res === null;
    },
    async text(testid, re, what) {
      const l = page.getByTestId(testid).first();
      let got = '';
      const ok = await l
        .waitFor({ state: 'visible', timeout: 4000 })
        .then(async () => {
          for (let i = 0; i < 20; i++) {
            got = (await l.innerText()).trim();
            if (re.test(got)) return true;
            await page.waitForTimeout(100);
          }
          return false;
        })
        .catch(() => false);
      t.check(ok, what ?? `${testid} reads ${re}`, `got "${got}"`);
      return ok;
    },
    async count(selector, expected, what) {
      let got = -1;
      for (let i = 0; i < 30; i++) {
        got = await page.locator(selector).count();
        if (got === expected) break;
        await page.waitForTimeout(100);
      }
      t.check(got === expected, what ?? `${expected} × ${selector}`, `found ${got}`);
      return got === expected;
    },
    /** Every invoke the window sent so far, as the fake saw it. */
    calls: () => page.evaluate(() => window.__fleetFake.calls.map((c) => ({ cmd: c.cmd, fleet: c.args?.fleet ?? null }))),
    /** Change the world while the window runs (see fake-hubs.js). */
    fake: (fn, arg) => page.evaluate(fn, arg),
    settle: (ms = 600) => page.waitForTimeout(ms),
  };

  console.log(`▶ ${sc.name}: ${sc.title}`);
  try {
    await page.goto(base);
    await page.getByTestId('pane-sidebar').waitFor({ state: 'visible', timeout: 15000 });
    await t.settle(800);
    await sc.run(t);
  } catch (e) {
    t.check(false, 'scenario ran to the end', String(e?.stack ?? e).split('\n').slice(0, 4).join(' | '));
    await t.shot('crash').catch(() => {});
  }
  const fakeState = await page
    .evaluate(() => ({ unknown: [...window.__fleetFake.unknown].sort(), violations: window.__fleetFake.violations }))
    .catch(() => ({ unknown: [], violations: [] }));
  entry.unknown = fakeState.unknown;
  if (fakeState.violations.length)
    t.check(false, 'every fleet-routed call is FLEET_SCOPED', fakeState.violations.join(', '));
  // Uncaught page errors are a failure; console noise is only reported.
  const pageErrors = entry.errors.filter((e) => e.startsWith('pageerror'));
  if (pageErrors.length) t.check(false, 'no uncaught page errors', pageErrors.join(' | '));
  entry.ok = entry.checks.every((c) => c.ok);
  if (!entry.ok) failed++;
  await context.close();
}
await browser.close();
await server.close();

// ── report ───────────────────────────────────────────────────────────────
writeFileSync(join(outDir, 'report.json'), JSON.stringify(report, null, 2));
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
const html = `<!doctype html><meta charset="utf-8"><title>Federation e2e</title>
<style>body{font:14px system-ui;margin:24px;background:#fafafa;color:#111}h2{margin-top:40px}
.ok{color:#15803d}.bad{color:#b91c1c}figure{display:inline-block;margin:8px;vertical-align:top;width:480px}
img{width:480px;border:1px solid #ddd}figcaption{font-size:12px;color:#555}ul{margin:4px 0}</style>
<h1>Federation e2e — ${report.length - failed}/${report.length} scenarios passed</h1>
${report
  .map(
    (r) => `<h2 class="${r.ok ? 'ok' : 'bad'}">${r.ok ? '✓' : '✗'} ${esc(r.name)}</h2><p>${esc(r.title)}</p>
<ul>${r.checks.map((c) => `<li class="${c.ok ? 'ok' : 'bad'}">${c.ok ? '✓' : '✗'} ${esc(c.what)}${c.detail ? ` — ${esc(c.detail)}` : ''}</li>`).join('')}</ul>
${r.shots.map((s) => `<figure><a href="${esc(s.file)}"><img src="${esc(s.file)}" loading="lazy"></a><figcaption>${esc(s.label)}</figcaption></figure>`).join('')}`,
  )
  .join('\n')}`;
writeFileSync(join(outDir, 'index.html'), html);

console.log(`\n${report.length - failed}/${report.length} scenarios passed · screenshots in ${relative(process.cwd(), outDir) || '.'}/index.html`);
process.exit(failed ? 1 : 0);
