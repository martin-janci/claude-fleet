// The TS twin of crates/fleet-core/src/pages/forms.rs `check_answers`:
// which steps and fields are asked, and what is wrong with an answer. Both
// run docs/form-examples/answers.json, so the words must match exactly.
import { optionsOfField, type FieldCondition, type FieldProblem, type FormField, type FormSpec, type FormStep, type Values } from './forms';
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
