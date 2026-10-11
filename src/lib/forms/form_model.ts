// The TS twin of crates/fleet-core/src/pages/forms.rs `check_answers`:
// which steps and fields are asked, and what is wrong with an answer. Both
// run docs/form-examples/answers.json, so the words must match exactly.
import { optionsOfField, type FieldCondition, type HostCheck, type FieldProblem, type FormField, type FormSpec, type FormStep, type Values } from './forms';
import { risky } from '../quick_answer';
export type { FormSpec, FormStep, FormField, FieldProblem, Values } from './forms';

const TEXT_LEN = 500;
const TEXTAREA_LEN = 5000;
const SECRET_LEN = 2000;

/** An own property only: a field may be named `constructor`. */
function own(o: Values, k: string): unknown {
  return Object.prototype.hasOwnProperty.call(o, k) ? o[k] : undefined;
}

function same(a: unknown, b: unknown): boolean {
  return a === b;
}

export function holds(c: FieldCondition | undefined, shown: Values): boolean {
  if (!c) return true;
  if (c.all) return c.all.every((x) => holds(x, shown));
  if (c.any) return c.any.some((x) => holds(x, shown));
  if (c.not) return !holds(c.not, shown);
  if (c.field === undefined) return true;
  const got = own(shown, c.field);
  const matches = (want: unknown) =>
    Array.isArray(got) ? got.some((g) => same(g, want)) : got !== undefined && same(got, want);
  if (c.eq !== undefined) return matches(c.eq);
  if (c.in !== undefined) return c.in.some(matches);
  if (c.truthy !== undefined) return (got === true) === c.truthy;
  return true;
}

function isBlank(v: unknown): boolean {
  return v === undefined || v === null || (typeof v === 'string' && v.trim() === '') || (Array.isArray(v) && v.length === 0);
}

function optionValues(f: FormField): string[] {
  return optionsOfField(f).map((o) => o.value);
}

/** `v` as an answer to `f`, normalised, or what is wrong with it. */
function checkValue(f: FormField, v: unknown): { ok: true; value: unknown } | { ok: false; problem: string } {
  switch (f.type) {
    case 'text':
    case 'textarea':
    case 'secret': {
      if (typeof v !== 'string') return { ok: false, problem: 'must be text' };
      const cap = f.type === 'text' ? (f.max_len ?? TEXT_LEN) : f.type === 'textarea' ? (f.max_len ?? TEXTAREA_LEN) : SECRET_LEN;
      if ([...v].length > cap) return { ok: false, problem: `is longer than ${cap} characters` };
      return { ok: true, value: v };
    }
    case 'number': {
      if (typeof v !== 'number' || !Number.isFinite(v)) return { ok: false, problem: 'must be a number' };
      if (f.integer && !Number.isInteger(v)) return { ok: false, problem: 'must be a whole number' };
      if (f.min !== undefined && v < f.min) return { ok: false, problem: `must be at least ${f.min}` };
      if (f.max !== undefined && v > f.max) return { ok: false, problem: `must be at most ${f.max}` };
      return { ok: true, value: v };
    }
    case 'bool':
      return typeof v === 'boolean' ? { ok: true, value: v } : { ok: false, problem: 'must be on or off' };
    case 'select':
      if (typeof v === 'string' && optionValues(f).includes(v)) return { ok: true, value: v };
      // "Another…": any text the person typed.
      if (f.other) {
        if (typeof v !== 'string') return { ok: false, problem: 'must be one of the options or your own text' };
        if ([...v].length > TEXT_LEN) return { ok: false, problem: `is longer than ${TEXT_LEN} characters` };
        return { ok: true, value: v };
      }
      return { ok: false, problem: 'must be one of the options' };
    case 'multiselect': {
      const bad = { ok: false as const, problem: 'must be a list of the options' };
      if (!Array.isArray(v) || !v.every((x) => typeof x === 'string')) return bad;
      const picked = new Set(v as string[]);
      if (picked.size !== v.length || ![...picked].every((p) => optionValues(f).includes(p))) return bad;
      return { ok: true, value: optionValues(f).filter((o) => picked.has(o)) };
    }
  }
}

interface Walk {
  answers: Values;
  secrets: string[];
  problems: FieldProblem[];
  steps: FormStep[];
}

function walk(spec: FormSpec, values: Values, onlyStep?: number): Walk {
  const out: Walk = { answers: {}, secrets: [], problems: [], steps: [] };
  spec.steps.forEach((step) => {
    if (!holds(step.when, out.answers)) return;
    const shown: FormField[] = [];
    const index = out.steps.length;
    for (const f of step.fields ?? []) {
      if (!holds(f.when, out.answers)) continue;
      shown.push(f);
      // Disabled: shown with its reason, never answered.
      if (f.disabled_reason !== undefined) continue;
      const report = onlyStep === undefined || onlyStep === index;
      const given = own(values, f.name);
      if (isBlank(given)) {
        if (f.required && report) out.problems.push({ field: f.name, problem: 'is required' });
        continue;
      }
      const r = checkValue(f, given);
      if (!r.ok) {
        if (report) out.problems.push({ field: f.name, problem: r.problem });
      } else if (r.value === false && f.required) {
        if (report) out.problems.push({ field: f.name, problem: 'is required' });
      } else if (f.type === 'secret') {
        out.secrets.push(f.name);
      } else {
        out.answers[f.name] = r.value;
      }
    }
    out.steps.push({ ...step, fields: shown });
  });
  return out;
}

export function visibleSteps(spec: FormSpec, values: Values): FormStep[] {
  return walk(spec, values).steps;
}

export function stepProblems(spec: FormSpec, stepIndex: number, values: Values): FieldProblem[] {
  return walk(spec, values, stepIndex).problems;
}

export function checkAnswers(
  spec: FormSpec,
  values: Values,
): { ok: true; answers: Values; secrets: string[] } | { ok: false; problems: FieldProblem[] } {
  const w = walk(spec, values);
  const known = new Set(spec.steps.flatMap((s) => (s.fields ?? []).map((f) => f.name)));
  for (const k of Object.keys(values).sort()) {
    if (!known.has(k)) w.problems.push({ field: k, problem: 'is not a field of this form' });
  }
  if (w.problems.length > 0) return { ok: false, problems: w.problems };
  return { ok: true, answers: w.answers, secrets: w.secrets.sort() };
}

/**
 * The values a form starts with: each field's `value`, never a secret's.
 * For an agent's form (`fromAgent`) the agent does not decide for the
 * person (review r09, as the phone does): a required checkbox starts
 * unticked, and no checkbox, choice or multi-choice option whose label names
 * a risky step (`RISKY_WORDS`: push, approve, allow, production…) starts
 * chosen. The app's own wizards keep their defaults.
 */
export function startingValues(spec: FormSpec, fromAgent: boolean): Values {
  const out: Values = {};
  for (const step of spec.steps)
    for (const f of step.fields ?? []) {
      if (f.type === 'secret' || f.value === undefined) continue;
      const v = fromAgent ? agentDefault(f, f.value) : f.value;
      if (v !== undefined) out[f.name] = v;
    }
  return out;
}

function agentDefault(f: FormField, v: unknown): unknown {
  const optionRisky = (value: unknown) => {
    const o = optionsOfField(f).find((x) => x.value === value);
    return risky(o ? o.label : String(value));
  };
  switch (f.type) {
    case 'bool':
      return v === true && (f.required || risky(f.label)) ? false : v;
    case 'select':
      return optionRisky(v) ? undefined : v;
    case 'multiselect':
      return Array.isArray(v) ? v.filter((x) => !optionRisky(x)) : v;
    default:
      return v;
  }
}

/**
 * Why a conditional step or field is on screen, in words (the Components
 * board's "Shown because ‘Needs a database’ is on"): its `when` read back
 * with the labels of the fields it names. Null for an unconditional one,
 * or a condition too tangled to say plainly (a `not` of a group).
 */
export function shownBecause(c: FieldCondition | undefined, spec: FormSpec): string | null {
  const words = conditionWords(c, spec);
  return words ? `Shown because ${words}` : null;
}

function fieldNamed(spec: FormSpec, name: string): FormField | undefined {
  for (const s of spec.steps) for (const f of s.fields ?? []) if (f.name === name) return f;
  return undefined;
}

function conditionWords(c: FieldCondition | undefined, spec: FormSpec, negate = false): string | null {
  if (!c) return null;
  if (c.all || c.any) {
    const parts = (c.all ?? c.any ?? []).map((x) => conditionWords(x, spec, negate));
    if (parts.length === 0 || parts.some((p) => p === null)) return null;
    // "not (a and b)" is not "not a and not b": leave a negated group unsaid.
    if (negate) return null;
    return parts.join(c.all ? ' and ' : ' or ');
  }
  if (c.not) return negate ? conditionWords(c.not, spec) : conditionWords(c.not, spec, true);
  if (c.field === undefined) return null;
  const f = fieldNamed(spec, c.field);
  const name = `‘${f?.label ?? c.field}’`;
  const said = (v: unknown) => {
    const o = f ? optionsOfField(f).find((x) => x.value === v) : undefined;
    return o ? `‘${o.label}’` : typeof v === 'string' ? `‘${v}’` : String(v);
  };
  const many = f?.type === 'multiselect';
  const is = many ? (negate ? 'does not include' : 'includes') : negate ? 'is not' : 'is';
  if (c.truthy !== undefined) return `${name} is ${c.truthy !== negate ? 'on' : 'off'}`;
  if (c.eq !== undefined) {
    if (typeof c.eq === 'boolean') return `${name} is ${c.eq !== negate ? 'on' : 'off'}`;
    return `${name} ${is} ${said(c.eq)}`;
  }
  if (c.in !== undefined) {
    const vs = c.in.map(said);
    if (vs.length === 0) return null;
    if (vs.length === 1) return `${name} ${is} ${vs[0]}`;
    if (negate) return `${name} ${many ? 'includes' : 'is'} none of ${vs.join(', ')}`;
    return `${name} ${is} ${vs.slice(0, -1).join(', ')} or ${vs[vs.length - 1]}`;
  }
  return null;
}

/** The host facts a check reads (a `HostRow`'s last health probe). */
export interface HostFacts {
  disk_home_free_kb?: number | null;
  mem_avail_kb?: number | null;
}

const KB_PER_GB = 1024 * 1024;

function gbWords(gb: number): string {
  return `${gb >= 10 ? Math.round(gb) : Math.round(gb * 10) / 10} GB`;
}

/**
 * The spec's host checks that the answers fall short of (the Components
 * board's "Postgres needs 2 GB free, mercury has 1.4 GB"): each check whose
 * `when` holds, on the host its `host_field` names (else `defaultHost`),
 * whose last probe says less than it needs. A host never probed, or a fact
 * it did not report, is no warning: the check says only what it knows.
 */
export function hostCheckWarnings(
  spec: FormSpec,
  values: Values,
  defaultHost: string | null | undefined,
  factsOf: (alias: string) => HostFacts | undefined,
): string[] {
  const shown = walk(spec, values).answers;
  const out: string[] = [];
  for (const c of spec.checks ?? ([] as HostCheck[])) {
    if (!holds(c.when, shown)) continue;
    const picked = c.host_field !== undefined ? own(values, c.host_field) : undefined;
    const host = typeof picked === 'string' && picked !== '' ? picked : (defaultHost ?? null);
    if (!host) continue;
    const facts = factsOf(host);
    const kb = c.needs === 'disk_free_gb' ? facts?.disk_home_free_kb : facts?.mem_avail_kb;
    if (kb === undefined || kb === null) continue;
    const has = kb / KB_PER_GB;
    if (has >= c.at_least) continue;
    const what = c.needs === 'disk_free_gb' ? 'free' : 'of memory free';
    out.push(`${c.label} needs ${gbWords(c.at_least)} ${what}, ${host} has ${gbWords(has)}.`);
  }
  return out;
}

/**
 * `spec` with `values` written in as drafted defaults (G7.4: Control drafts
 * a form from a free-text message): each value lands on the field of its
 * name as its `value`, marked Drafted by `by` from `from`, when the field
 * takes it as an answer. A secret, a disabled field, an unknown name or a
 * value the field would refuse (an option it does not offer) is dropped,
 * so the person fills that one in.
 */
export function withDrafted(spec: FormSpec, values: Values, by: string, from: string): FormSpec {
  return {
    ...spec,
    steps: spec.steps.map((st) => ({
      ...st,
      fields: (st.fields ?? []).map((f) => {
        const v = own(values, f.name);
        if (v === undefined || f.type === 'secret' || f.disabled_reason !== undefined || isBlank(v)) return f;
        const r = checkValue(f, v);
        return r.ok ? { ...f, value: r.value, drafted: { by, from } } : f;
      }),
    })),
  };
}
