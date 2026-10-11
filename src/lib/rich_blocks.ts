// Rich blocks in a reply: the parts of an assistant's text that the
// Conversation view draws as a card instead of as Markdown. Three sources:
//
// - A task report: a `FLEET_TASK_DONE_<nonce>` line followed by one JSON
//   object (fenced or bare), the shape crates/fleet-core/src/service/work/report.rs
//   asks a run for. Normalised exactly as `report_from_value` does there.
// - A `fleet.ui/1` block: a fenced ```fleet-ui block (or a ```json block
//   whose object says `"spec": "fleet.ui/1"`) holding one display block —
//   report, steps, guide, callout, facts, choices, form, progress, results
//   or error. docs/chat-blocks.md is the format; the Rust twin of the check
//   is crates/fleet-core/src/pages/chat_blocks.rs, and both run
//   docs/chat-block-examples/blocks.json.
// - A work handover: the text between `WORK_HANDOVER_BEGIN_<nonce>` and
//   `WORK_HANDOVER_END_<nonce>` lines, the shape agent_handover.rs asks a
//   session for; handover.ts reads it into sections.
//
// Everything here is data from an untrusted transcript: nothing is ever
// interpreted as HTML, and a block that does not check out falls back to the
// code block it was written as, with what is wrong under it.
import type { FormField, FormSpec } from './forms/forms';
import { CHAT_WIZARD_IDS, type ChatWizardId } from './forms/chat_wizard_ids';
import { fenceOpen, isClosingFence } from './markdown';
import { HANDOVER_BEGIN, handoverMarker, parseHandover, type Handover } from './handover';

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

export const PROGRESS_STATES = ['running', 'waiting', 'done', 'failed'] as const;
export type ProgressState = (typeof PROGRESS_STATES)[number];
export const PROGRESS_STEP_STATES = ['pending', 'running', 'done', 'failed', 'skipped'] as const;
export type ProgressStepState = (typeof PROGRESS_STEP_STATES)[number];
export interface ProgressStep {
  title: string;
  state: ProgressStepState;
  /** One line under the step while it runs: "attempt 2 of 10" (G7.4). */
  detail?: string;
}

/** What an error's next step may do itself instead of filling the
 *  composer (G7.4; chat_blocks.rs `NEXT_ACTIONS`). */
export const NEXT_ACTIONS = ['login', 'open_host', 'open_accounts'] as const;
export type NextAction = (typeof NEXT_ACTIONS)[number];
const NEXT_ACTIONS_ON_HOST: readonly string[] = ['login', 'open_host'];
/** An error's next step: a choice, or with `action` one the app runs. */
export interface UiNextStep extends UiChoice {
  action?: NextAction;
  /** The host a `login` or `open_host` acts on. */
  host?: string;
}

/** A value's type in a results card: the page data sources' column types
 *  (pages.ts `ColType`), formatted by the same `formatCell`. */
export const RESULT_TYPES = ['text', 'int', 'tokens', 'usd_micros', 'day', 'time'] as const;
export type ResultType = (typeof RESULT_TYPES)[number];
export interface ResultAxis {
  label: string;
  ty?: ResultType;
}
export type ResultCell = string | number | boolean | null;
export const RESULT_CHARTS = ['line', 'bar', 'sparkline'] as const;
export type ResultChart = (typeof RESULT_CHARTS)[number];
export type ResultItem =
  | { type: 'stat'; label: string; value: number | string; ty?: ResultType; hint?: string }
  | { type: 'chart'; chart: ResultChart; title: string; x: ResultAxis; y: ResultAxis; points: [string | number, number][] }
  | { type: 'table'; title?: string; columns: ResultAxis[]; rows: ResultCell[][] };
export const RESULT_ITEM_TYPES = ['stat', 'chart', 'table'] as const;

export type UiBlock =
  | ({ kind: 'report'; title?: string } & TaskReport)
  | { kind: 'steps'; title: string; intro?: string; steps: UiStep[] }
  /** With `page`, a guide fleet already has, drawn as Settings draws it
   *  (`rich/GuidePageCard.svelte`); `title` and `sections` are then empty. */
  | { kind: 'guide'; page?: string; title: string; intro?: string; sections: UiSection[] }
  | { kind: 'callout'; tone: Tone; title?: string; body: string }
  | { kind: 'facts'; title?: string; items: [string, string][] }
  | { kind: 'choices'; title?: string; question?: string; options: UiChoice[] }
  | { kind: 'form'; form: FormSpec }
  /** A long job's state. Blocks with the same `id` in one conversation are
   *  one card that updates in place (`rich/progress_board.ts`). */
  | {
      kind: 'progress';
      id: string;
      title: string;
      state: ProgressState;
      done?: number;
      total?: number;
      unit?: string;
      steps?: ProgressStep[];
      note?: string;
      /** When the job started (unix seconds): the card counts the time since. */
      started_at?: number;
    }
  | { kind: 'results'; title?: string; summary?: string; items: ResultItem[] }
  | { kind: 'error'; code: string; title: string; body?: string; detail?: string; next: UiNextStep[] }
  /** A settings change waiting for a person: the id `set_setting` with
   *  `propose: true` answered. The card reads the key and values from the
   *  proposal, never from the block (`rich/SettingCard.svelte`). */
  | { kind: 'setting'; proposal: number; note?: string }
  /** One of the app's own wizards as a form in the chat (step 10.12): the
   *  spec is the app's, never the block's (`forms/WizardChatCard.svelte`). */
  | { kind: 'wizard'; wizard: ChatWizardId; why?: string; values?: Record<string, string | number | boolean> };

export type UiKind = UiBlock['kind'];
export const UI_KINDS: UiKind[] = ['report', 'steps', 'guide', 'callout', 'facts', 'choices', 'form', 'progress', 'results', 'error', 'setting', 'wizard'];
/** A progress `id`, an error `code` and a guide's `page`: a key, never prose. */
const KEY_RE = /^[A-Za-z0-9_.:-]+$/;
const KEY_MAX = 64;

export type RichSegment =
  | { t: 'md'; source: string }
  /** `marker` is the done line; `raw` the JSON text it was read from. */
  | { t: 'report'; marker: string; report: TaskReport; raw: string }
  /** `raw` is the fence body, also the block's identity for its card state. */
  | { t: 'ui'; block: UiBlock; raw: string }
  /** `raw` is the text between the markers, as written. */
  | { t: 'handover'; nonce: string; handover: Handover; raw: string }
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
  if (
    !source.includes('FLEET_TASK_DONE_') &&
    !source.includes(UI_FENCE) &&
    !source.includes(UI_SPEC) &&
    !source.includes(HANDOVER_BEGIN)
  ) {
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
    const h = handoverAfter(lines, i);
    if (h) {
      flush();
      out.push(h.seg);
      i = h.end + 1;
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

/** The handover opening on line `i`: its BEGIN marker, then the first END
 *  marker with the same nonce. Until that line arrives (the reply is still
 *  being written) the text stays Markdown; an empty hand-off is no card. */
function handoverAfter(lines: string[], i: number): { seg: RichSegment; end: number } | null {
  const open = handoverMarker(lines[i]);
  if (!open || open.end) return null;
  for (let k = i + 1; k < lines.length; k++) {
    const m = handoverMarker(lines[k]);
    if (!m) continue;
    if (!m.end || m.nonce !== open.nonce) return null;
    const raw = lines.slice(i + 1, k).join('\n').trim();
    if (raw === '') return null;
    return { seg: { t: 'handover', nonce: open.nonce, handover: parseHandover(raw), raw }, end: k };
  }
  return null;
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

function num(
  o: Record<string, unknown>,
  key: string,
  p: Problems,
  where: string,
  opts: { min: number },
): number | undefined {
  const v = o[key];
  if (v === undefined || v === null) return undefined;
  if (typeof v !== 'number' || !Number.isFinite(v)) {
    p.add(where, `\`${key}\` must be a number`);
    return undefined;
  }
  if (!Number.isInteger(v)) p.add(where, `\`${key}\` must be a whole number`);
  else if (v < opts.min) p.add(where, `\`${key}\` must be at least ${opts.min}`);
  return v;
}

function oneOf<T extends string>(
  o: Record<string, unknown>,
  key: string,
  p: Problems,
  where: string,
  allowed: readonly T[],
  fallback?: T,
): T | undefined {
  const v = o[key] ?? fallback;
  if (v === undefined) {
    p.add(where, `\`${key}\` is required`);
    return undefined;
  }
  if (typeof v !== 'string' || !(allowed as readonly string[]).includes(v)) {
    p.add(where, `\`${key}\` must be one of ${allowed.join(', ')}`);
    return undefined;
  }
  return v as T;
}

function key(o: Record<string, unknown>, name: string, p: Problems, where: string): string {
  const v = str(o, name, p, where, { required: true, max: KEY_MAX }) ?? '';
  if (v !== '' && !KEY_RE.test(v)) p.add(where, `\`${name}\` must be letters, digits and . _ : -`);
  return v;
}

/** Only when the key is there: an optional list. */
function optArr(o: Record<string, unknown>, k: string, p: Problems, where: string, min: number, max: number): unknown[] | undefined {
  return o[k] === undefined || o[k] === null ? undefined : arr(o, k, p, where, min, max);
}

function axis(v: unknown, p: Problems, where: string): ResultAxis {
  if (!isObject(v)) {
    p.add(where, 'must be an object');
    return { label: '' };
  }
  return {
    label: str(v, 'label', p, where, { required: true, max: 80 }) ?? '',
    ty: v.ty === undefined || v.ty === null ? undefined : oneOf(v, 'ty', p, where, RESULT_TYPES),
  };
}

const isCell = (c: unknown): c is ResultCell =>
  c === null || typeof c === 'string' || typeof c === 'boolean' || (typeof c === 'number' && Number.isFinite(c));

function choice(o: Record<string, unknown>, p: Problems, at: string): UiChoice {
  return {
    label: str(o, 'label', p, at, { required: true, max: 80 }) ?? '',
    prompt: str(o, 'prompt', p, at, { required: true, max: 4000 }) ?? '',
    hint: str(o, 'hint', p, at, { max: 200 }),
  };
}

/** A `wizard` block's drafted answers (G7.4; chat_blocks.rs `drafted_values`). */
function draftedValues(v: unknown, p: Problems): Record<string, string | number | boolean> | undefined {
  if (v === undefined || v === null) return undefined;
  if (!isObject(v) || Array.isArray(v)) {
    p.add('', '`values` must be an object of answers by field name');
    return undefined;
  }
  const keys = Object.keys(v);
  if (keys.length > 20) p.add('', '`values` has more than 20 answers');
  const out: Record<string, string | number | boolean> = {};
  for (const k of keys.sort()) {
    const at = `values › ${k}`;
    if (!/^[a-z][a-z0-9_]{0,39}$/.test(k)) p.add(at, 'must be a field name: lowercase letters, digits and _');
    const x = v[k];
    if (typeof x === 'string' && [...x].length > 500) p.add(at, 'is longer than 500 characters');
    else if (typeof x === 'string' || typeof x === 'boolean' || (typeof x === 'number' && Number.isFinite(x))) out[k] = x;
    else p.add(at, 'must be text, a number or true/false');
  }
  return out;
}

function nextStep(o: Record<string, unknown>, p: Problems, at: string): UiNextStep {
  const c = choice(o, p, at);
  const action = o.action === undefined || o.action === null ? undefined : oneOf(o, 'action', p, at, NEXT_ACTIONS);
  const host = action && NEXT_ACTIONS_ON_HOST.includes(action) ? key(o, 'host', p, at) : undefined;
  return { ...c, ...(action ? { action } : {}), ...(host ? { host } : {}) };
}

function resultItem(o: Record<string, unknown>, p: Problems, at: string): ResultItem | null {
  switch (o.type) {
    case 'stat': {
      const label = str(o, 'label', p, at, { required: true, max: 80 }) ?? '';
      const v = o.value;
      let value: number | string = '';
      if (typeof v === 'number' && Number.isFinite(v)) value = v;
      else if (typeof v === 'string' && [...v].length <= 80) value = v;
      else p.add(at, '`value` must be a number or text of at most 80 characters');
      const ty = o.ty === undefined || o.ty === null ? undefined : oneOf(o, 'ty', p, at, RESULT_TYPES);
      return { type: 'stat', label, value, ty, hint: str(o, 'hint', p, at, { max: 200 }) };
    }
    case 'chart': {
      const chart = oneOf(o, 'chart', p, at, RESULT_CHARTS) ?? 'bar';
      const title = str(o, 'title', p, at, { required: true, max: 120 }) ?? '';
      const x = axis(o.x, p, `${at} › x`);
      const y = axis(o.y, p, `${at} › y`);
      const points = arr(o, 'points', p, at, 1, 200).flatMap((pt, k): [string | number, number][] => {
        if (
          Array.isArray(pt) &&
          pt.length === 2 &&
          (typeof pt[0] === 'string' || (typeof pt[0] === 'number' && Number.isFinite(pt[0]))) &&
          typeof pt[1] === 'number' &&
          Number.isFinite(pt[1])
        )
          return [[pt[0], pt[1]]];
        p.add(`${at} › point ${k + 1}`, 'must be [x, number]');
        return [];
      });
      return { type: 'chart', chart, title, x, y, points };
    }
    case 'table': {
      const title = str(o, 'title', p, at, { max: 120 });
      const columns = arr(o, 'columns', p, at, 1, 12).map((c, k) => axis(c, p, `${at} › column ${k + 1}`));
      const rows = arr(o, 'rows', p, at, 0, 200).flatMap((r, k): ResultCell[][] => {
        const rat = `${at} › row ${k + 1}`;
        if (!Array.isArray(r)) {
          p.add(rat, 'must be a list');
          return [];
        }
        if (r.length !== columns.length) p.add(rat, `has ${r.length} cells for ${columns.length} columns`);
        r.forEach((c, j) => {
          if (!isCell(c)) p.add(rat, `cell ${j + 1} must be text, a number, a bool or null`);
        });
        return [r as ResultCell[]];
      });
      return { type: 'table', title, columns, rows };
    }
    default:
      p.add(at, `\`type\` must be one of ${RESULT_ITEM_TYPES.join(', ')}`);
      return null;
  }
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
    // A review step (forms.rs `StepKind::Review`) has no fields.
    const review = s.kind === 'review';
    const fs = review && s.fields === undefined ? [] : arr(s, 'fields', p, at, review ? 0 : 1, 40);
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
          // [value, label], or {value, label, …} (forms.rs `FormOption`).
          const pair = Array.isArray(o) && o.length === 2 && typeof o[0] === 'string' && typeof o[1] === 'string';
          const obj = isObject(o) && typeof o.value === 'string' && typeof o.label === 'string';
          if (!pair && !obj)
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
      if (v.page !== undefined && v.page !== null) {
        block = { kind: 'guide', page: key(v, 'page', p, ''), title: '', sections: [] };
        break;
      }
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
      const options = each(arr(v, 'options', p, '', 1, 8), p, 'option', (o, at) => choice(o, p, at));
      block = { kind: 'choices', title: str(v, 'title', p, '', { max: 120 }), question: str(v, 'question', p, '', { max: 500 }), options };
      break;
    }
    case 'form': {
      const form = checkForm(v.form, p);
      if (form) block = { kind: 'form', form };
      break;
    }
    case 'progress': {
      const id = key(v, 'id', p, '');
      const title = str(v, 'title', p, '', { required: true, max: 120 }) ?? '';
      const state = oneOf(v, 'state', p, '', PROGRESS_STATES, 'running') ?? 'running';
      const done = num(v, 'done', p, '', { min: 0 });
      const total = num(v, 'total', p, '', { min: 1 });
      if (done !== undefined && total !== undefined && done > total) p.add('', '`done` is more than `total`');
      const stepList = optArr(v, 'steps', p, '', 1, 20);
      const steps =
        stepList &&
        each(stepList, p, 'step', (s, at) => ({
          title: str(s, 'title', p, at, { required: true, max: 200 }) ?? '',
          state: oneOf(s, 'state', p, at, PROGRESS_STEP_STATES, 'pending') ?? 'pending',
          detail: str(s, 'detail', p, at, { max: 120 }),
        }));
      const unit = str(v, 'unit', p, '', { max: 20 });
      const note = str(v, 'note', p, '', { max: 2000 });
      const started_at = num(v, 'started_at', p, '', { min: 0 });
      block = { kind: 'progress', id, title, state, done, total, unit, steps, note, started_at };
      break;
    }
    case 'results': {
      const title = str(v, 'title', p, '', { max: 120 });
      const summary = str(v, 'summary', p, '', { max: 2000 });
      const items = each(arr(v, 'items', p, '', 1, 12), p, 'item', (o, at) => resultItem(o, p, at)).filter(
        (x): x is ResultItem => x !== null,
      );
      block = { kind: 'results', title, summary, items };
      break;
    }
    case 'error': {
      const code = key(v, 'code', p, '');
      const title = str(v, 'title', p, '', { required: true, max: 120 }) ?? '';
      const next = each(optArr(v, 'next', p, '', 1, 4) ?? [], p, 'next', (o, at) => nextStep(o, p, at));
      block = { kind: 'error', code, title, body: str(v, 'body', p, '', { max: 4000 }), detail: str(v, 'detail', p, '', { max: 8000 }), next };
      break;
    }
    case 'setting': {
      let proposal: number | undefined;
      if (v.proposal === undefined || v.proposal === null) p.add('', '`proposal` is required');
      else proposal = num(v, 'proposal', p, '', { min: 1 });
      block = { kind: 'setting', proposal: proposal ?? 0, note: str(v, 'note', p, '', { max: 500 }) };
      break;
    }
    case 'wizard': {
      const wizard = oneOf(v, 'wizard', p, '', CHAT_WIZARD_IDS);
      const why = str(v, 'why', p, '', { max: 500 });
      const values = draftedValues(v.values, p);
      if (wizard) block = { kind: 'wizard', wizard, why, ...(values ? { values } : {}) };
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
