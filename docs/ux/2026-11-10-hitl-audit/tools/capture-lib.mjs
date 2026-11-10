// Shared capture library for the UX audit (NOT part of the repo).
// Runs the real desktop frontend in Chromium against the repo's fake Tauri IPC
// (e2e/federation/fake-hubs.js) plus your own extra command answers.
//
//   import { capture } from '<scratchpad>/capture/lib.mjs';
//   await capture({
//     outDir: '<scratchpad>/d1',
//     cacheDir: '<scratchpad>/d1/vite-cache',   // one per agent
//     runs: [{
//       name: 'inbox', world: {...}, prefs: {...}, theme: 'dark'|'light',
//       viewport: {width:1440,height:900}, extraFile: '<abs path to extra-fake.js>',
//       async steps(t) { await t.click('rail-inbox'); await t.shot('inbox-populated'); }
//     }]
//   });
//
// extraFile is plain browser JS injected AFTER the fake. It must set
//   window.__FLEET_EXTRA__ = { command_name: (args, fake) => answer | undefined }
// A handler returning undefined falls through to the repo's fake. `fake` is
// window.__fleetFake (emit(event,payload), hubs, calls, unknown ...).
import { createRequire } from 'node:module';
import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

export const ROOT = '/home/user/claude-fleet';
const HARNESS = join(ROOT, 'e2e/federation');

const req = createRequire(join(ROOT, 'package.json'));
const { chromium } = req(req.resolve('playwright', { paths: ['/opt/node-tools/node_modules'] }));

const WRAP = `(() => {
  const inner = window.__TAURI_INTERNALS__.invoke;
  window.__FLEET_EXTRA__ = window.__FLEET_EXTRA__ || {};
  window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
    const h = window.__FLEET_EXTRA__[cmd];
    if (h) {
      const r = await h(args, window.__fleetFake);
      if (r !== undefined) { window.__fleetFake.calls.push({ cmd, args: args ?? null, at: Date.now(), extra: true }); return r; }
    }
    return inner(cmd, args);
  };
})();`;

const BASE_PREFS = { 'onboarding-welcomed': true, 'onboarding-dismissed': true, 'hints-enabled': false, 'tour-seen': true };

export async function capture({ outDir, cacheDir, runs, only = null }) {
  const shotsDir = join(outDir, 'shots');
  mkdirSync(shotsDir, { recursive: true });
  const fleetsTs = readFileSync(join(ROOT, 'src/lib/fleets.ts'), 'utf8');
  const scoped = [...fleetsTs.match(/export const FLEET_SCOPED = \[([\s\S]*?)\] as const/)[1].matchAll(/'([a-z_]+)'/g)].map((m) => m[1]);
  const { createServer } = await import(pathToFileURL(join(ROOT, 'node_modules/vite/dist/node/index.js')).href);
  const server = await createServer({ configFile: join(HARNESS, 'vite.config.mjs'), cacheDir, server: { port: 0 } });
  await server.listen();
  const base = server.resolvedUrls.local[0];
  const shim = readFileSync(join(HARNESS, 'fake-hubs.js'), 'utf8');
  const browser = await chromium.launch({ headless: true });
  const manifestPath = join(outDir, 'manifest.json');
  const manifest = existsSync(manifestPath) ? JSON.parse(readFileSync(manifestPath, 'utf8')) : [];
  const report = [];
  try {
    for (const run of runs) {
      if (only && !run.name.includes(only)) continue;
      const theme = run.theme ?? 'dark';
      const viewport = run.viewport ?? { width: 1440, height: 900 };
      const entry = { run: run.name, theme, viewport, shots: [], errors: [], unknown: [] };
      report.push(entry);
      const context = await browser.newContext({ viewport, deviceScaleFactor: run.scale ?? 2, colorScheme: theme });
      const prefs = { ...BASE_PREFS, ...(run.prefs ?? {}) };
      await context.addInitScript({
        content:
          `window.__FLEET_WORLD__ = ${JSON.stringify(run.world)};\n` +
          `window.__FLEET_SCOPED__ = ${JSON.stringify(scoped)};\n` +
          `try { localStorage.setItem('cf:theme', ${JSON.stringify(theme)});` +
          ` for (const [k, v] of Object.entries(${JSON.stringify(prefs)})) localStorage.setItem('cf:pref:' + k, JSON.stringify(v)); } catch {}`,
      });
      await context.addInitScript({ content: shim });
      if (run.extraFile) await context.addInitScript({ content: readFileSync(run.extraFile, 'utf8') });
      await context.addInitScript({ content: WRAP });
      const page = await context.newPage();
      page.on('pageerror', (e) => entry.errors.push(`pageerror: ${e.message}`));
      page.on('console', (m) => { if (m.type() === 'error') entry.errors.push(`console: ${m.text().slice(0, 300)}`); });
      const t = {
        page,
        byTestId: (id) => page.getByTestId(id),
        settle: (ms = 500) => page.waitForTimeout(ms),
        async click(testid) { await page.getByTestId(testid).first().click(); await page.waitForTimeout(300); },
        async key(k) { await page.keyboard.press(k); await page.waitForTimeout(300); },
        fake: (fn, arg) => page.evaluate(fn, arg),
        /** Save shots/<run>--<label>-<theme>.png and add a manifest row. `meta`: {screen, state, reached}. */
        async shot(label, meta = {}) {
          await page.waitForTimeout(300);
          const file = `${run.name}--${label}-${theme}${viewport.width !== 1440 ? '-w' + viewport.width : ''}.png`;
          await page.screenshot({ path: join(shotsDir, file), fullPage: !!meta.fullPage });
          const row = { file, run: run.name, label, theme, viewport, ...meta };
          entry.shots.push(file);
          const i = manifest.findIndex((m) => m.file === file);
          if (i >= 0) manifest[i] = row; else manifest.push(row);
        },
        /** Screenshot one element only (a card, a row, a dialog). */
        async shotOf(locator, label, meta = {}) {
          await page.waitForTimeout(200);
          const file = `${run.name}--${label}-${theme}.png`;
          await locator.first().screenshot({ path: join(shotsDir, file) });
          const row = { file, run: run.name, label, theme, element: true, ...meta };
          entry.shots.push(file);
          const i = manifest.findIndex((m) => m.file === file);
          if (i >= 0) manifest[i] = row; else manifest.push(row);
        },
      };
      console.log(`▶ ${run.name} (${theme} ${viewport.width}x${viewport.height})`);
      try {
        await page.goto(base);
        await page.getByTestId('pane-sidebar').waitFor({ state: 'visible', timeout: 20000 });
        await t.settle(800);
        await run.steps(t);
      } catch (e) {
        entry.errors.push('steps: ' + String(e?.stack ?? e).split('\n').slice(0, 4).join(' | '));
        await t.shot('CRASH').catch(() => {});
      }
      entry.unknown = await page.evaluate(() => [...window.__fleetFake.unknown].sort()).catch(() => []);
      console.log(`  ${entry.shots.length} shots, ${entry.errors.length} errors, ${entry.unknown.length} unanswered commands`);
      await context.close();
    }
  } finally {
    await browser.close();
    await server.close();
    writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
    writeFileSync(join(outDir, 'report.json'), JSON.stringify(report, null, 2));
  }
  return report;
}
