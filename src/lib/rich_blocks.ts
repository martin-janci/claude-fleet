// Rich blocks in a reply: the parts of an assistant's text that the
// Conversation view draws as a card instead of as Markdown. Two sources:
//
// - A task report: a `FLEET_TASK_DONE_<nonce>` line followed by one JSON
//   object (fenced or bare), the shape crates/fleet-core/src/service/work/report.rs
//   asks a run for. Normalised exactly as `report_from_value` does there.
// - A `fleet.ui/1` block: a fenced ```fleet-ui block (or a ```json block
//   whose object says `"spec": "fleet.ui/1"`) holding one display block —
//   report, steps, guide, callout, facts, choices or form. docs/chat-blocks.md
//   is the format.
//
// Everything here is data from an untrusted transcript: nothing is ever
// interpreted as HTML, and a block that does not check out falls back to the
// code block it was written as, with what is wrong under it.
import type { FormField, FormSpec } from './forms/forms';
import { fenceOpen, isClosingFence } from './markdown';

export const UI_SPEC = 'fleet.ui/1';
/** The fence language that marks a block as a card. */
export const UI_FENCE = 'fleet-ui';
/** Largest block JSON drawn as a card (bytes of the fence body). */
export const UI_MAX_BYTES = 32 * 1024;

// Mirrors crates/fleet-core/src/store/task_report.rs and report.rs.
export const REPORT_OUTCOMES = ['done', 'partial', 'blocked', 'failed'] as const;
export type ReportOutcome = (typeof REPORT_OUTCOMES)[number];
const REPORT_LIST_MAX = 20;
const REPORT_ENTRY_MAX_CHARS = 500;
const SUMMARY_MAX_CHARS = 4000;
/** Most lines after a marker searched for its JSON, as report.rs. */
const REPORT_SCAN_LINES = 400;

export interface TaskReport {
  summary: string;
  outcome: ReportOutcome;
  tests_run: string[];
  warnings: string[];
  blockers: string[];
  followups: string[];
  confidence: string | null;
}

export type Tone = 'info' | 'tip' | 'success' | 'warning' | 'danger';
export const TONES: Tone[] = ['info', 'tip', 'success', 'warning', 'danger'];

export interface UiStep {
  title: string;
  body?: string;
  code?: string;
  lang?: string;
}
export interface UiSection {
  title: string;
  body: string;
}
export interface UiChoice {
  label: string;
  prompt: string;
  hint?: string;
}

export type UiBlock =
  | ({ kind: 'report'; title?: string } & TaskReport)
  | { kind: 'steps'; title: string; intro?: string; steps: UiStep[] }
  | { kind: 'guide'; title: string; intro?: string; sections: UiSection[] }
  | { kind: 'callout'; tone: Tone; title?: string; body: string }
  | { kind: 'facts'; title?: string; items: [string, string][] }
  | { kind: 'choices'; title?: string; question?: string; options: UiChoice[] }
  | { kind: 'form'; form: FormSpec };

export type UiKind = UiBlock['kind'];
export const UI_KINDS: UiKind[] = ['report', 'steps', 'guide', 'callout', 'facts', 'choices', 'form'];

export type RichSegment =
  | { t: 'md'; source: string }
  /** `marker` is the done line; `raw` the JSON text it was read from. */
  | { t: 'report'; marker: string; report: TaskReport; raw: string }
  /** `raw` is the fence body, also the block's identity for its card state. */
  | { t: 'ui'; block: UiBlock; raw: string }
  /** A fleet-ui block that does not check out: drawn as its code, plus why. */
  | { t: 'invalid'; lang: string; raw: string; problems: string[] };

// ── Reports ────────────────────────────────────────────────────────────────

const clip = (s: string, max: number) => [...s.trim()].slice(0, max).join('');

function reportList(v: unknown): string[] {
  if (v === undefined || v === null) return [];
  const items = Array.isArray(v) ? v : [v];
  return items
    .filter((x) => x !== null && x !== undefined)
    .map((x) => clip(typeof x === 'string' ? x : JSON.stringify(x), REPORT_ENTRY_MAX_CHARS))
    .filter((s) => s.length > 0)
    .slice(0, REPORT_LIST_MAX);
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

/** A report from the JSON a worker printed, capped as report.rs caps it.
 *  `null` when the value is not an object. */
export function reportFromValue(v: unknown): TaskReport | null {
  if (!isObject(v)) return null;
  const raw = typeof v.outcome === 'string' ? v.outcome.trim().toLowerCase() : '';
  const outcome = (REPORT_OUTCOMES as readonly string[]).includes(raw) ? (raw as ReportOutcome) : 'partial';
  const c = v.confidence;
  const confidence =
    typeof c === 'string' && c.trim() !== '' ? clip(c, 20) : typeof c === 'number' && Number.isFinite(c) ? clip(String(c), 20) : null;
  return {
    summary: typeof v.summary === 'string' ? clip(v.summary, SUMMARY_MAX_CHARS) : '',
    outcome,
    tests_run: reportList(v.tests_run),
    warnings: reportList(v.warnings),
    blockers: reportList(v.blockers),
    followups: reportList(v.followups),
    confidence,
  };
}

// ── Lines and fences ───────────────────────────────────────────────────────

const MARKER_RE = /^FLEET_TASK_DONE_[A-Za-z0-9_]+$/;

/** The REPL's chrome before assistant text (`⏺ `, `│ `), as report.rs strips it. */
function unchrome(line: string): string {
  const t = line.trimStart();
  const s = t.startsWith('⏺') || t.startsWith('│') ? t.slice(1) : t;
  return s.trimStart();
}

/** The done marker on this line, or null. Leading punctuation (`⏺`, `**`)
 *  is allowed, as `after_marker` allows it. */
export function markerOf(line: string): string | null {
  const bare = line
    .trim()
    .replace(/^[^A-Za-z0-9_]+/, '')
    .replace(/[^A-Za-z0-9_]+$/, '');
  return MARKER_RE.test(bare) ? bare : null;
}

interface Fence {
  /** Index of the opening line. */
  open: number;
  /** Index of the closing line; lines.length when the fence never closes. */
  close: number;
  lang: string;
  body: string;
}

/** The fence opening on line `i`, read with markdown.ts's own rules so a
 *  card and the Markdown agree on where code starts and ends. */
function fenceAt(lines: string[], i: number): Fence | null {
  const f = fenceOpen(lines[i]);
  if (!f) return null;
  let close = lines.length;
  for (let k = i + 1; k < lines.length; k++) {
    if (isClosingFence(lines[k], f.marker)) {
      close = k;
      break;
    }
  }
  return { open: i, close, lang: f.lang.toLowerCase(), body: lines.slice(i + 1, close).join('\n') };
}

function parseJson(s: string): { ok: true; value: unknown } | { ok: false } {
  try {
    return { ok: true, value: JSON.parse(s) };
  } catch {
    return { ok: false };
  }
}

/** The report following a marker on line `i`: the first fenced block or bare
 *  `{…}` after blank lines. Returns the report and the last line it used. */
function reportAfter(lines: string[], i: number): { report: TaskReport; raw: string; end: number } | null {
  let k = i + 1;
  while (k < lines.length && lines[k].trim() === '') k++;
  if (k >= lines.length) return null;
  const f = fenceAt(lines, k);
  if (f) {
    if (f.close >= lines.length) return null; // still streaming
    const p = parseJson(f.body.trim());
    const report = p.ok ? reportFromValue(p.value) : null;
    return report ? { report, raw: f.body.trim(), end: f.close } : null;
  }
  if (!unchrome(lines[k]).startsWith('{')) return null;
  // A bare object: the shortest run of lines from `{` that parses.
  const last = Math.min(lines.length, k + REPORT_SCAN_LINES);
  for (let e = k; e < last; e++) {
    if (!unchrome(lines[e]).trimEnd().endsWith('}')) continue;
    const text = lines
      .slice(k, e + 1)
      .map(unchrome)
      .join('\n');
    const p = parseJson(text);
    if (!p.ok) continue;
    const report = reportFromValue(p.value);
    return report ? { report, raw: text, end: e } : null;
  }
  return null;
}

/**
 * Split an assistant text block into Markdown runs and cards. A fence that
 * never closes (the reply is still being written) stays Markdown, so a block
 * turns into its card only once it is whole.
 */
export function splitRich(source: string): RichSegment[] {
  if (!source.includes('FLEET_TASK_DONE_') && !source.includes(UI_FENCE) && !source.includes(UI_SPEC)) {
    return [{ t: 'md', source }];
  }
  const lines = source.replace(/\r\n?/g, '\n').split('\n');
  const out: RichSegment[] = [];
  let md: string[] = [];
  const flush = () => {
    const s = md.join('\n');
    if (s.trim() !== '') out.push({ t: 'md', source: s });
    md = [];
  };
  let i = 0;
  while (i < lines.length) {
    const f = fenceAt(lines, i);
    if (f) {
      const closed = f.close < lines.length;
      const end = Math.min(f.close, lines.length - 1);
      const seg = closed ? uiSegment(f) : null;
      if (seg) {
        flush();
        out.push(seg);
      } else {
        // Any other fence is passed through whole, so a marker or a
        // fleet-ui fence quoted inside a code block stays code.
        md.push(...lines.slice(i, end + 1));
      }
      i = end + 1;
      continue;
    }
    const marker = markerOf(lines[i]);
    if (marker) {
      const r = reportAfter(lines, i);
      if (r) {
        flush();
        out.push({ t: 'report', marker, report: r.report, raw: r.raw });
        i = r.end + 1;
        continue;
      }
    }
    md.push(lines[i]);
    i++;
  }
  flush();
  return out.length > 0 ? out : [{ t: 'md', source }];
}

/** The card a closed fence stands for, or null when it is ordinary code. */
function uiSegment(f: Fence): RichSegment | null {
  const raw = f.body.trim();
  const tagged = f.lang === UI_FENCE;
  if (!tagged) {
    // A ```json block is a card only when it says it is one.
    if (f.lang !== 'json' || !raw.startsWith('{') || !raw.includes(UI_SPEC)) return null;
    const p = parseJson(raw);
    if (!p.ok || !isObject(p.value) || p.value.spec !== UI_SPEC) return null;
  }
  const checked = checkUiBlock(raw);
  return checked.ok ? { t: 'ui', block: checked.block, raw } : { t: 'invalid', lang: f.lang, raw, problems: checked.problems };
}

// ── fleet.ui/1 ─────────────────────────────────────────────────────────────

type Check = { ok: true; block: UiBlock } | { ok: false; problems: string[] };

class Problems {
  list: string[] = [];
  add(where: string, what: string) {
    if (this.list.length < 20) this.list.push(where ? `${where}: ${what}` : what);
  }
}

function str(o: Record<string, unknown>, key: string, p: Problems, where: string, opts: { required?: boolean; max: number }): string | undefined {
  const v = o[key];
  if (v === undefined || v === null) {
    if (opts.required) p.add(where, `\`${key}\` is required`);
    return undefined;
  }
  if (typeof v !== 'string') {
    p.add(where, `\`${key}\` must be text`);
    return undefined;
  }
  if (opts.required && v.trim() === '') p.add(where, `\`${key}\` is empty`);
  if ([...v].length > opts.max) p.add(where, `\`${key}\` is longer than ${opts.max} characters`);
  return v;
}

function arr(o: Record<string, unknown>, key: string, p: Problems, where: string, min: number, max: number): unknown[] {
  const v = o[key];
  if (!Array.isArray(v)) {
    p.add(where, `\`${key}\` must be a list`);
    return [];
  }
  if (v.length < min) p.add(where, `\`${key}\` needs at least ${min} ${min === 1 ? 'entry' : 'entries'}`);
  if (v.length > max) p.add(where, `\`${key}\` has more than ${max} entries`);
  return v.slice(0, max);
}

function each<T>(items: unknown[], p: Problems, where: string, f: (o: Record<string, unknown>, at: string) => T): T[] {
  return items.flatMap((x, k) => {
    const at = `${where} ${k + 1}`;
    if (!isObject(x)) {
      p.add(at, 'must be an object');
      return [];
    }
    return [f(x, at)];
  });
}

const FIELD_TYPES_SHOWN = new Set(['text', 'textarea', 'number', 'bool', 'select', 'multiselect']);
const NAME_RE = /^[a-z][a-z0-9_]{0,39}$/;

/** Enough of fleet.form/1 for FormWizard to draw it safely. The backend's
 *  full validator (pages/forms.rs) is for `ask`; a form in a reply only
 *  fills the composer, so a looser check costs nothing. A secret is refused:
 *  its answer would land in the transcript. */
function checkForm(v: unknown, p: Problems): FormSpec | null {
  const where = 'form';
  if (!isObject(v)) {
    p.add(where, 'must be a fleet.form/1 object');
    return null;
  }
  if (v.spec !== 'fleet.form/1') p.add(where, '`spec` must be "fleet.form/1"');
  str(v, 'title', p, where, { required: true, max: 120 });
  str(v, 'intro', p, where, { max: 500 });
  str(v, 'submit', p, where, { max: 40 });
  const names = new Set<string>();
  let fields = 0;
  const steps = arr(v, 'steps', p, where, 1, 12);
  each(steps, p, 'form › step', (s, at) => {
    str(s, 'title', p, at, { required: true, max: 120 });
    str(s, 'intro', p, at, { max: 500 });
    const fs = arr(s, 'fields', p, at, 1, 40);
    each(fs, p, `${at} › field`, (f, fat) => {
      fields++;
      const name = str(f, 'name', p, fat, { required: true, max: 40 });
      if (name !== undefined) {
        if (!NAME_RE.test(name)) p.add(fat, `name "${name}" must be lowercase letters, digits and _`);
        if (names.has(name)) p.add(fat, `name "${name}" appears twice`);
        names.add(name);
      }
      str(f, 'label', p, fat, { required: true, max: 200 });
      str(f, 'help', p, fat, { max: 500 });
      str(f, 'placeholder', p, fat, { max: 200 });
      const type = f.type;
      if (type === 'secret') p.add(fat, 'a secret field is only for `ask`: its answer would land in the transcript');
      else if (typeof type !== 'string' || !FIELD_TYPES_SHOWN.has(type)) p.add(fat, `type ${JSON.stringify(type)} is not a field type`);
      if (type === 'select' || type === 'multiselect') {
        const opts = arr(f, 'options', p, fat, 1, 50);
        opts.forEach((o, k) => {
          if (!Array.isArray(o) || o.length !== 2 || typeof o[0] !== 'string' || typeof o[1] !== 'string')
            p.add(`${fat} › option ${k + 1}`, 'must be [value, label]');
        });
      }
      for (const k of ['min', 'max', 'max_len'] as const)
        if (f[k] !== undefined && (typeof f[k] !== 'number' || !Number.isFinite(f[k]))) p.add(fat, `\`${k}\` must be a number`);
      return null as FormField | null;
    });
    return null;
  });
  if (fields > 40) p.add(where, 'has more than 40 fields');
  return p.list.length === 0 ? (v as unknown as FormSpec) : null;
}

/** A fleet.ui/1 block from its JSON text, or every problem with it. */
export function checkUiBlock(raw: string): Check {
  const p = new Problems();
  if (new TextEncoder().encode(raw).length > UI_MAX_BYTES) return { ok: false, problems: [`is larger than ${UI_MAX_BYTES / 1024} KiB`] };
  const parsed = parseJson(raw);
  if (!parsed.ok) return { ok: false, problems: ['is not valid JSON'] };
  const v = parsed.value;
  if (!isObject(v)) return { ok: false, problems: ['must be a JSON object'] };
  if (v.spec !== UI_SPEC) p.add('', `\`spec\` must be "${UI_SPEC}"`);
  const kind = v.kind;
  if (typeof kind !== 'string' || !(UI_KINDS as string[]).includes(kind)) {
    p.add('', `\`kind\` must be one of ${UI_KINDS.join(', ')}`);
    return { ok: false, problems: p.list };
  }
  let block: UiBlock | null = null;
  switch (kind as UiKind) {
    case 'report': {
      const title = str(v, 'title', p, '', { max: 120 });
      const r = reportFromValue(v);
      if (r) block = { kind: 'report', title, ...r };
      break;
    }
    case 'steps': {
      const title = str(v, 'title', p, '', { required: true, max: 120 }) ?? '';
      const intro = str(v, 'intro', p, '', { max: 2000 });
      const steps = each(arr(v, 'steps', p, '', 1, 30), p, 'step', (s, at) => ({
        title: str(s, 'title', p, at, { required: true, max: 200 }) ?? '',
        body: str(s, 'body', p, at, { max: 4000 }),
        code: str(s, 'code', p, at, { max: 8000 }),
        lang: str(s, 'lang', p, at, { max: 20 }),
      }));
      block = { kind: 'steps', title, intro, steps };
      break;
    }
    case 'guide': {
      const title = str(v, 'title', p, '', { required: true, max: 120 }) ?? '';
      const intro = str(v, 'intro', p, '', { max: 2000 });
      const sections = each(arr(v, 'sections', p, '', 1, 20), p, 'section', (s, at) => ({
        title: str(s, 'title', p, at, { required: true, max: 200 }) ?? '',
        body: str(s, 'body', p, at, { required: true, max: 8000 }) ?? '',
      }));
      block = { kind: 'guide', title, intro, sections };
      break;
    }
    case 'callout': {
      const tone = v.tone ?? 'info';
      if (typeof tone !== 'string' || !(TONES as string[]).includes(tone)) p.add('', `\`tone\` must be one of ${TONES.join(', ')}`);
      block = {
        kind: 'callout',
        tone: tone as Tone,
        title: str(v, 'title', p, '', { max: 120 }),
        body: str(v, 'body', p, '', { required: true, max: 4000 }) ?? '',
      };
      break;
    }
    case 'facts': {
      const items = arr(v, 'items', p, '', 1, 40).flatMap((x, k): [string, string][] => {
        if (Array.isArray(x) && x.length === 2 && typeof x[0] === 'string' && (typeof x[1] === 'string' || typeof x[1] === 'number' || typeof x[1] === 'boolean'))
          return [[clip(x[0], 120), clip(String(x[1]), 1000)]];
        p.add(`item ${k + 1}`, 'must be [label, value]');
        return [];
      });
      block = { kind: 'facts', title: str(v, 'title', p, '', { max: 120 }), items };
      break;
    }
    case 'choices': {
      const options = each(arr(v, 'options', p, '', 1, 8), p, 'option', (o, at) => ({
        label: str(o, 'label', p, at, { required: true, max: 80 }) ?? '',
        prompt: str(o, 'prompt', p, at, { required: true, max: 4000 }) ?? '',
        hint: str(o, 'hint', p, at, { max: 200 }),
      }));
      block = { kind: 'choices', title: str(v, 'title', p, '', { max: 120 }), question: str(v, 'question', p, '', { max: 500 }), options };
      break;
    }
    case 'form': {
      const form = checkForm(v.form, p);
      if (form) block = { kind: 'form', form };
      break;
    }
  }
  return p.list.length === 0 && block ? { ok: true, block } : { ok: false, problems: p.list.length ? p.list : ['is not a block'] };
}

// ── What a card puts in the composer ───────────────────────────────────────

/** The prompt a submitted reply form fills the composer with: the form's
 *  title and the answers as one JSON block, so the agent reads them back
 *  without guessing at prose. */
export function formAnswerPrompt(form: FormSpec, values: Record<string, unknown>): string {
  return `Answers to the form "${form.title}":\n\n\`\`\`json\n${JSON.stringify(values, null, 2)}\n\`\`\``;
}

/** A short stable key for a block's text: a card's local state (ticked
 *  steps, a sent form) survives the transcript being re-read. */
export function blockKey(raw: string): string {
  let h = 5381;
  for (let i = 0; i < raw.length; i++) h = ((h << 5) + h + raw.charCodeAt(i)) | 0;
  return (h >>> 0).toString(36);
}

/** `raw` as a fenced code block whose fence no run of backticks inside it
 *  can close. */
export function fenced(lang: string, raw: string): string {
  const longest = Math.max(2, ...(raw.match(/`+/g) ?? []).map((r) => r.length));
  const bar = '`'.repeat(longest + 1);
  return `${bar}${lang}\n${raw}\n${bar}`;
}
