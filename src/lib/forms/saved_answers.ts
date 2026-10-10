// "Save and finish later" (a form whose spec says `save_later`): what the
// person typed into a pending form, kept on this device until they come
// back, answer it or decline it. Never a secret: only the visible,
// non-secret values are kept. The hub's `form_drafts` table (migration 153)
// is the agent's spec while it is being written, purged after 10 minutes
// and cleared when the form opens, so it is not a place for answers.
import { visibleSteps } from './form_model';
import { optionsOfField, type FormSpec, type Values } from './forms';

const PREFIX = 'fleet.form.saved.';

/** The answers kept for `key`, or null. Storage can throw (a private
 *  window, blocked site data): then nothing is kept. */
export function loadSaved(key: string): Values | null {
  try {
    const raw = localStorage.getItem(PREFIX + key);
    if (!raw) return null;
    const v = JSON.parse(raw) as unknown;
    return v && typeof v === 'object' && !Array.isArray(v) ? (v as Values) : null;
  } catch {
    return null;
  }
}

/** What of `kept` still fits `spec` as it opens now: a field it still has,
 *  and for a choice an option it still offers (the fleet's hosts or
 *  projects may have changed since), or own text where it takes some. */
export function restorable(spec: FormSpec, kept: Values): Values {
  const fields = new Map(spec.steps.flatMap((s) => s.fields ?? []).map((f) => [f.name, f] as const));
  const out: Values = {};
  for (const [name, v] of Object.entries(kept)) {
    const f = fields.get(name);
    if (!f || f.type === 'secret' || f.disabled_reason !== undefined) continue;
    const offered = new Set(optionsOfField(f).map((o) => o.value));
    if (f.type === 'select' && !(typeof v === 'string' && (offered.has(v) || f.other))) continue;
    if (f.type === 'multiselect') {
      if (!Array.isArray(v)) continue;
      out[name] = v.filter((x) => typeof x === 'string' && offered.has(x));
      continue;
    }
    out[name] = v;
  }
  return out;
}

/** The values worth keeping: on a visible field, not a secret, not disabled. */
export function keepable(spec: FormSpec, values: Values): Values {
  const out: Values = {};
  for (const s of visibleSteps(spec, values))
    for (const f of s.fields) {
      if (f.type === 'secret' || f.disabled_reason !== undefined) continue;
      if (Object.prototype.hasOwnProperty.call(values, f.name) && values[f.name] !== undefined) out[f.name] = values[f.name];
    }
  return out;
}

/** Keep `values` for `key`; whether it was kept. */
export function saveForLater(key: string, spec: FormSpec, values: Values): boolean {
  try {
    localStorage.setItem(PREFIX + key, JSON.stringify(keepable(spec, values)));
    return true;
  } catch {
    return false;
  }
}

export function clearSaved(key: string): void {
  try {
    localStorage.removeItem(PREFIX + key);
  } catch {
    // Nothing kept, nothing to clear.
  }
}
