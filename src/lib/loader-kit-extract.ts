// Turns the design manual's Loader board (`preview.html`) and its stylesheet
// (`bundle.css`) into the app's loader kit: `loader-kit.generated.ts` (the 24
// loaders' markup) and `loader-kit.generated.css` (their animations).
//
// The manual stays the source. `loader-kit.test.ts` re-runs this on the
// files in docs/ux and fails when the generated pair drifts; regenerate with
//   REGEN_LOADERS=1 pnpm exec vitest run src/lib/loader-kit.test.ts
// (the run writes both files, then fails on purpose: read the diff, run again).
//
// Every class, keyframe and id is prefixed `ofl-` so the kit cannot collide
// with the app's own `.spin`, `.dot` or `.chip`, and every rule is scoped
// under `.ofl`, the Loader's root.

export type LoaderKind = 'logo' | 'particle';

export interface LoaderSpec {
  /** Kebab-case name, the Loader's `name` prop. */
  id: string;
  /** The manual's name for it. */
  name: string;
  kind: LoaderKind;
  /** The one job the manual gives it. */
  job: string;
  /** Natural box in px; the Loader scales it to `size`. */
  width: number;
  height: number;
  /** Markup inside the Loader's box (manual's, prefixed). */
  markup: string;
}

/** The first twelve cards are the LogoMotion board, the rest the Loaders board. */
const LOGO_COUNT = 12;

/** Natural boxes of the particle loaders, from their bundle.css rules. */
const NATURAL: Record<string, [number, number]> = {
  sw: [200, 200],
  cv: [160, 160],
  rd2: [150, 150],
  so: [170, 170],
  dw: [190, 118],
  rn: [196, 150],
  gx: [190, 150],
  gw: [150, 150],
  l10: [230, 64],
};

/**
 * Keyframes the bundle defines twice. In the board the later `br` (the
 * Atom's nucleus) replaces the first (Breathe's glow), so Breathe loses its
 * glow; the kit gives each its own name.
 */
const SPLIT_KEYFRAMES: { name: string; selector: RegExp; renamed: string }[] = [
  { name: 'br', selector: /^\.nuc\b/, renamed: 'nuc' },
];

const slug = (s: string) =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '');

const prefixClasses = (list: string) =>
  list
    .split(/\s+/)
    .filter(Boolean)
    .map((c) => `ofl-${c}`)
    .join(' ');

/** Keyframe names used in an `animation` / `animation-name` value, prefixed. */
function renameAnimations(decls: string, names: Set<string>, rename: Record<string, string> = {}): string {
  return decls.replace(/(animation(?:-name)?\s*:)([^;}"]*)/g, (_m, prop: string, value: string) => {
    const v = value.replace(/[A-Za-z_][\w-]*/g, (w) => {
      if (rename[w]) return `ofl-${rename[w]}`;
      return names.has(w) ? `ofl-${w}` : w;
    });
    return prop + v;
  });
}

/** Keyframe names defined in the loader section of bundle.css. */
function keyframeNames(css: string): Set<string> {
  return new Set([...css.matchAll(/@keyframes\s+([\w-]+)/g)].map((m) => m[1]));
}

/** The bundle's loader section: from the satellite origin rule to Liquid orbit. */
export function loaderSection(bundle: string): string {
  const start = bundle.indexOf('.mo svg .sat');
  const endRule = bundle.indexOf('.gw .go i{');
  if (start < 0 || endRule < 0) throw new Error('bundle.css: loader section markers not found');
  const end = bundle.indexOf('}', endRule) + 1;
  return bundle.slice(start, end);
}

/** Splits a flat stylesheet into top-level blocks (`sel{…}` or `@x{…{…}}`). */
function blocks(css: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let from = 0;
  for (let i = 0; i < css.length; i++) {
    if (css[i] === '{') depth++;
    else if (css[i] === '}' && --depth === 0) {
      out.push(css.slice(from, i + 1).trim());
      from = i + 1;
    }
  }
  return out.filter(Boolean);
}

/** Selectors the kit leaves to the hand-written loader-kit.css. */
const DROP = /^(\.mo\s*\.(grid|card|cap|stage|pct)\b|@media|@keyframes\s+fadeq\b)/;

export function extractCss(bundle: string): string {
  const section = loaderSection(bundle);
  const names = keyframeNames(section);
  const out: string[] = [];
  let splitSeen: Record<string, number> = {};
  for (const block of blocks(section)) {
    if (DROP.test(block)) continue;
    const kf = block.match(/^@keyframes\s+([\w-]+)\s*\{([\s\S]*)\}$/);
    if (kf) {
      const name = kf[1];
      splitSeen[name] = (splitSeen[name] ?? 0) + 1;
      const split = SPLIT_KEYFRAMES.find((s) => s.name === name);
      const final = split && splitSeen[name] > 1 ? split.renamed : name;
      out.push(`@keyframes ofl-${final}{${kf[2]}}`);
      continue;
    }
    const brace = block.indexOf('{');
    const selector = block.slice(0, brace).trim();
    const decls = block.slice(brace);
    const rename: Record<string, string> = {};
    for (const s of SPLIT_KEYFRAMES) if (s.selector.test(selector)) rename[s.name] = s.renamed;
    const scoped = selector
      .split(',')
      .map((sel) => {
        const t = sel.trim().replace(/^\.mo\s+/, '');
        return '.ofl ' + t.replace(/\.([A-Za-z][\w-]*)/g, '.ofl-$1');
      })
      .join(',');
    out.push(scoped + renameAnimations(decls, names, rename).replace(/url\(#goo\)/g, 'url(#ofl-goo)'));
  }
  return out.join('\n') + '\n';
}

export function extractLoaders(preview: string, bundle: string): LoaderSpec[] {
  const names = keyframeNames(loaderSection(bundle));
  const cards = [
    ...preview.matchAll(
      /<div class="card"[^>]*><div class="stage">([\s\S]*?)<\/div><div class="cap"><b>([^<]*)<\/b><span>([^<]*)<\/span>/g,
    ),
  ];
  if (cards.length !== 24) throw new Error(`preview.html: expected 24 loaders, found ${cards.length}`);
  return cards.map((m, i) => {
    const [, stage, name, job] = m;
    const id = slug(name);
    let markup = stage.replace(/^<div style="transform:scale\(\.62\)">/, '').replace(/<\/div>$/, '');
    let width: number;
    let height: number;
    if (id === 'comet') {
      // The board shows three sizes; the kit has one, sized by `--s`.
      markup = '<span class="cm"></span>';
      width = height = 32;
    } else {
      const top = markup.match(/^<svg width="(\d+)" height="(\d+)"/);
      const key = Object.keys(NATURAL).find((k) => new RegExp(`class="${k}"`).test(markup));
      if (key) [width, height] = NATURAL[key];
      else if (top) [width, height] = [Number(top[1]), Number(top[2])];
      else throw new Error(`preview.html: no natural size for ${name}`);
      if (i < LOGO_COUNT && top) {
        // The mark fills the Loader's box.
        markup = markup.replace(/^<svg width="\d+" height="\d+"/, '<svg width="100%" height="100%"');
      }
    }
    markup = markup
      // The Loader carries the accessible name; the drawing is decoration.
      .replace(/ role="img" aria-label="[^"]*"/g, '')
      .replace(/ class=""/g, '')
      // The percentage is the consumer's text, not the drawing's.
      .replace(/<span class="pct[^"]*">[^<]*<\/span>/g, '')
      .replace(/ id="goo"/g, ' id="ofl-goo"')
      .replace(/ class="([^"]*)"/g, (_m, list: string) => ` class="${prefixClasses(list)}"`)
      .replace(/ style="([^"]*)"/g, (_m, style: string) => ` style="${renameAnimations(style, names)}"`);
    return { id, name, kind: i < LOGO_COUNT ? 'logo' : 'particle', job, width, height, markup };
  });
}

export function renderLoadersModule(specs: LoaderSpec[]): string {
  const lines = [
    '// GENERATED by src/lib/loader-kit-extract.ts from the design manual\'s Loader',
    '// board (docs/ux/2026-10-08-orbit-fleet-redesign/design-system/components/Loader).',
    '// Do not edit; regenerate with',
    '//   REGEN_LOADERS=1 pnpm exec vitest run src/lib/loader-kit.test.ts',
    "import type { LoaderSpec } from './loader-kit-extract';",
    '',
    `export const LOADER_NAMES = ${JSON.stringify(specs.map((s) => s.id))} as const;`,
    '',
    'export type LoaderName = (typeof LOADER_NAMES)[number];',
    '',
    'export const LOADER_SPECS: readonly LoaderSpec[] = [',
  ];
  for (const s of specs) lines.push(`  ${JSON.stringify(s)},`);
  lines.push('];', '');
  return lines.join('\n');
}

export function renderLoadersCss(bundle: string): string {
  return (
    "/* GENERATED by src/lib/loader-kit-extract.ts from the design manual's bundle.css.\n" +
    ' * Do not edit; regenerate with\n' +
    ' *   REGEN_LOADERS=1 pnpm exec vitest run src/lib/loader-kit.test.ts */\n' +
    extractCss(bundle)
  );
}

// ---- counted loaders -------------------------------------------------------
//
// Two particle loaders can say how many: Radar (one blip per host that
// answers, step 3.15) and Assemble (one particle per session). The manual
// draws a fixed set; `countedMarkup` keeps the first `count` of them and, for
// Radar, places more blips than the board drew on the same sweep.

/** Loaders whose particles can follow a count, and the most they show. */
export const COUNTED: Readonly<Record<string, number>> = { radar: 24, assemble: 64 };

const PARTICLE = /<i style="[^"]*"><\/i>/g;

/** Radar's sweep period (bundle.css `ofl-blip`, `ofl-spin` on `.ofl-sweep`). */
const RADAR_SWEEP_S = 2.4;

/** A blip the board did not draw: the n-th on a golden-angle spiral, lit as
 *  the sweep passes it (the board's own blips trail their angle by ~0.57 s). */
function radarBlip(n: number): string {
  const angle = (n * 137.508) % 360;
  const r = 18 + ((n * 7) % 24);
  const rad = (angle * Math.PI) / 180;
  const left = (50 + r * Math.sin(rad)).toFixed(0);
  const top = (50 - r * Math.cos(rad)).toFixed(0);
  const delay = (((angle / 360) * RADAR_SWEEP_S - 0.57 + RADAR_SWEEP_S) % RADAR_SWEEP_S).toFixed(2);
  return `<i style="left:${left}%;top:${top}%;animation-delay:${delay}s;background:var(--done)"></i>`;
}

/** The loader's markup with one particle per item, up to COUNTED's cap. A
 *  loader that does not count gets its markup back unchanged. */
export function countedMarkup(spec: LoaderSpec, count: number): string {
  const cap = COUNTED[spec.id];
  if (cap === undefined) return spec.markup;
  const n = Math.max(0, Math.min(cap, Math.floor(count)));
  // The board repeats a blip to close its loop; one blip per item, so once.
  const drawn = [...new Set(spec.markup.match(PARTICLE) ?? [])];
  const kept = drawn.slice(0, n);
  if (spec.id === 'radar') for (let i = drawn.length; kept.length < n; i++) kept.push(radarBlip(i));
  const first = spec.markup.search(PARTICLE);
  if (first < 0) return spec.markup;
  const without = spec.markup.replace(PARTICLE, '');
  return without.slice(0, first) + kept.join('') + without.slice(first);
}
