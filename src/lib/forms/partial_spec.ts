// A fleet.form/1 spec while it is still being written (redesign step 10.12,
// the ChatWizards board's "Control builds it"): Control or an agent streams
// the JSON, and the card draws the title and each field as soon as it is
// whole. Pure, so the card and its tests read the same thing.
//
// The text is cut back to the last point where a value ended, the open
// brackets are closed, and what parses is kept. A field shows only once it
// has its name, type and label; a step only once it has a title.
import type { FieldType, FormField, FormSpec, FormStep } from './forms';

export interface PartialSpec {
  title: string | null;
  intro: string | null;
  /** The steps written so far, each with its whole fields. */
  steps: FormStep[];
  /** The text is the whole spec (it parses as written). */
  complete: boolean;
}

const TYPES: readonly FieldType[] = ['text', 'textarea', 'number', 'bool', 'select', 'multiselect', 'secret'];

/** The JSON prefix `text` would be with every open bracket closed, at each
 *  point where a value may end, latest first. */
function candidates(text: string): string[] {
  const out: string[] = [];
  const stack: string[] = [];
  let inString = false;
  let escaped = false;
  const close = () => [...stack].reverse().join('');
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (inString) {
      if (escaped) escaped = false;
      else if (c === '\\') escaped = true;
      else if (c === '"') inString = false;
      continue;
    }
    if (c === '"') inString = true;
    else if (c === '{') {
      stack.push('}');
      out.push(text.slice(0, i + 1) + close());
    } else if (c === '[') {
      stack.push(']');
      out.push(text.slice(0, i + 1) + close());
    } else if (c === '}' || c === ']') {
      stack.pop();
      out.push(text.slice(0, i + 1) + close());
    } else if (c === ',') out.push(text.slice(0, i) + close());
  }
  if (!inString) out.push(text + close());
  return out.reverse();
}

function str(v: unknown): string | null {
  return typeof v === 'string' && v.trim() !== '' ? v : null;
}

function wholeField(v: unknown): FormField | null {
  if (!v || typeof v !== 'object') return null;
  const f = v as Partial<FormField>;
  if (!str(f.name) || !str(f.label) || !TYPES.includes(f.type as FieldType)) return null;
  return f as FormField;
}

function shape(doc: unknown, complete: boolean): PartialSpec {
  const d = (doc && typeof doc === 'object' ? doc : {}) as Partial<FormSpec>;
  const steps: FormStep[] = [];
  for (const s of Array.isArray(d.steps) ? d.steps : []) {
    if (!s || typeof s !== 'object' || !str(s.title)) continue;
    const fields = (Array.isArray(s.fields) ? s.fields : []).map(wholeField).filter((f): f is FormField => f !== null);
    steps.push({ ...s, fields });
  }
  return { title: str(d.title), intro: str(d.intro), steps, complete };
}

/** What of the spec in `text` is written so far. */
export function partialSpec(text: string): PartialSpec {
  try {
    return shape(JSON.parse(text), true);
  } catch {
    // Still being written: read the longest prefix that parses.
  }
  for (const c of candidates(text)) {
    try {
      return shape(JSON.parse(c), false);
    } catch {
      // A cut inside a key or a number: try the next point back.
    }
  }
  return { title: null, intro: null, steps: [], complete: false };
}

/** The spec once the text is whole, else null: the card opens it then. */
export function finishedSpec(text: string): FormSpec | null {
  try {
    const doc = JSON.parse(text) as FormSpec;
    return doc && doc.spec === 'fleet.form/1' && Array.isArray(doc.steps) ? doc : null;
  } catch {
    return null;
  }
}
