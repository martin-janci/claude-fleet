// What a decided chat form collapses to (redesign step 10.1): one line for
// how it ended, one line summarising the answers, and the answers in full
// behind "Show answers". A pending form says when it expires. Pure, so the
// card and its tests read the same words.
import { visibleSteps } from './form_model';
import { optionsOfField, type FormField, type FormSpec, type FormView, type Values } from './forms';

/** A pending form nobody answered within this long expires. Mirrors
 *  `EXPIRE_SECS` in crates/fleet-core/src/service/forms.rs (receipt.test.ts
 *  reads it there). */
export const FORM_EXPIRE_SECS = 24 * 3600;

/** "expires in 9 min", "expires in 23 h", or "expires now" past the line. */
export function expiresIn(createdAt: number, nowSec: number): string {
  const left = createdAt + FORM_EXPIRE_SECS - nowSec;
  if (left < 60) return 'expires now';
  if (left < 3600) return `expires in ${Math.floor(left / 60)} min`;
  return `expires in ${Math.floor(left / 3600)} h`;
}

export interface Answer {
  name: string;
  label: string;
  /** The answer in words; a secret says where it went, never what it was. */
  text: string;
}

function optionLabel(f: FormField, v: unknown): string {
  return optionsOfField(f).find((o) => o.value === v)?.label ?? String(v);
}

function lower(label: string): string {
  return label.length > 1 && label[1] !== label[1].toUpperCase() ? label[0].toLowerCase() + label.slice(1) : label;
}

function oneLine(s: string, max = 40): string {
  const line = s.split('\n')[0].trim();
  return line.length > max ? line.slice(0, max - 1) + '…' : line;
}

/** The answered fields in form order, as the person gave them. */
export function answerList(form: FormView): Answer[] {
  const answers = form.answers ?? {};
  const secrets = form.secrets ?? {};
  const out: Answer[] = [];
  for (const step of visibleSteps(form.spec, answers)) {
    for (const f of step.fields) {
      if (f.type === 'secret') {
        if (f.name in secrets) out.push({ name: f.name, label: f.label, text: `written to ${form.host_alias}, never shown` });
        continue;
      }
      const text = answerText(f, answers[f.name]);
      if (text !== null) out.push({ name: f.name, label: f.label, text });
    }
  }
  return out;
}

/** One non-secret answer in words, or null when there is none. */
function answerText(f: FormField, v: unknown): string | null {
  if (v === undefined || v === null || v === '') return null;
  if (f.type === 'bool') return v === true ? 'yes' : 'no';
  if (f.type === 'select') return optionLabel(f, v);
  if (f.type === 'multiselect') return Array.isArray(v) && v.length > 0 ? v.map((x) => optionLabel(f, x)).join(', ') : null;
  return String(v);
}

/** One earlier step on a review step: its chip name, where Edit goes
 *  (the visible step's index), and what it holds so far. */
export interface ReviewSection {
  step: number;
  title: string;
  rows: Answer[];
}

/** What a review step (`kind: "review"`) summarises: every visible step
 *  before it, each field's answer in words. A secret says only whether it
 *  is set; a disabled field is left out (it is not answered); an empty
 *  field reads "—". */
export function reviewSections(spec: FormSpec, values: Values): ReviewSection[] {
  const out: ReviewSection[] = [];
  visibleSteps(spec, values).forEach((s, i) => {
    if (s.kind === 'review') return;
    const rows: Answer[] = [];
    for (const f of s.fields) {
      if (f.disabled_reason !== undefined) continue;
      const v = values[f.name];
      const text =
        f.type === 'secret'
          ? typeof v === 'string' && v !== ''
            ? 'set, never shown to the agent'
            : null
          : answerText(f, v);
      rows.push({ name: f.name, label: f.label, text: text ?? '—' });
    }
    out.push({ step: i, title: s.name ?? s.title, rows });
  });
  return out;
}

/** "papaya-receipts · Web app · macOS, Windows · 3 workers · database on ·
 *  1 secret": the Components board's receipt line. */
export function answerSummary(form: FormView): string {
  const answers = form.answers ?? {};
  const fields = new Map(visibleSteps(form.spec, answers).flatMap((s) => s.fields.map((f) => [f.name, f] as const)));
  const parts: string[] = [];
  let secrets = 0;
  for (const a of answerList(form)) {
    const f = fields.get(a.name)!;
    if (f.type === 'secret') secrets += 1;
    else if (f.type === 'bool') parts.push(`${lower(f.label)} ${answers[f.name] === true ? 'on' : 'off'}`);
    else if (f.type === 'number') parts.push(`${a.text} ${lower(f.label)}`);
    else parts.push(oneLine(a.text));
  }
  if (secrets) parts.push(`${secrets} secret${secrets === 1 ? '' : 's'}`);
  return parts.join(' · ');
}

const ENDED: Record<string, string> = {
  answered: 'answered',
  declined: 'declined',
  cancelled: 'withdrawn by the agent',
  expired: 'expired',
  pending: 'still waiting',
};

/** "answered by Martin", "declined", "withdrawn by the agent", "expired". */
export function endedWords(form: FormView): string {
  const by = form.answered_by && (form.state === 'answered' || form.state === 'declined') ? ` by ${form.answered_by}` : '';
  return `${ENDED[form.state] ?? form.state}${by}`;
}

/** The mark before the title: ✓ answered, ✕ declined, ◷ expired, – withdrawn. */
export function endedMark(state: string): string {
  return state === 'answered' ? '✓' : state === 'declined' ? '✕' : state === 'expired' ? '◷' : '–';
}
