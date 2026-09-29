// A plain-words settings command (declarative pages P5, design §5 "a
// natural-language palette"): "set recent work to 3 days", "turn on press
// enter", "disable auto-tidy". No model is involved: the words are matched
// against the registry's labels and keys, and the value is parsed by the
// setting's own kind, so only a declared key and a value it can hold ever
// come out. The result is a confirm row; nothing is written until the
// person presses Enter or Apply (and a setting that needs confirming asks
// first, as its field does).
import { fromDisplay, type Descriptor } from './pages';

export type NlResult =
  | { kind: 'change'; d: Descriptor; value: string }
  /** Several settings match the words equally well. */
  | { kind: 'ambiguous'; options: Descriptor[] }
  /** A command, but it cannot become a change; `d` when a setting matched. */
  | { kind: 'error'; message: string; d?: Descriptor };

const STOP = new Set(['the', 'a', 'an', 'of', 'for', 'to', 'setting', 'option']);
const ON = new Set(['on', 'true', 'yes', 'enable', 'enabled']);
const OFF = new Set(['off', 'false', 'no', 'disable', 'disabled']);

const SECS: [RegExp, number][] = [
  [/^(s|sec|secs|second|seconds)$/, 1],
  [/^(m|min|mins|minute|minutes)$/, 60],
  [/^(h|hr|hrs|hour|hours)$/, 3600],
  [/^(d|day|days)$/, 86_400],
  [/^(w|wk|wks|week|weeks)$/, 604_800],
];
const SECS_PER_UNIT: Partial<Record<Descriptor['unit'], number>> = {
  seconds: 1,
  minutes: 60,
  hours: 3600,
  days: 86_400,
};

const words = (s: string) =>
  s
    .toLowerCase()
    .replace(/[-_.]/g, ' ')
    .split(/[^a-z0-9%]+/)
    .filter((w) => w && !STOP.has(w));

/** Split a command into the setting's words and the value's, or null when
 *  the query is not a command (then it is an ordinary search). */
export function splitCommand(query: string): { phrase: string; value: string } | null {
  const q = query.trim().replace(/\s+/g, ' ');
  let m = /^(?:set|change|make)\s+(.+?)\s+(?:to|=)\s+(.+)$/i.exec(q);
  if (m) return { phrase: m[1], value: m[2] };
  m = /^(?:turn|switch)\s+(on|off)\s+(.+)$/i.exec(q);
  if (m) return { phrase: m[2], value: m[1] };
  m = /^(?:turn|switch)\s+(.+)\s+(on|off)$/i.exec(q);
  if (m) return { phrase: m[1], value: m[2] };
  m = /^(enable|disable)\s+(.+)$/i.exec(q);
  if (m) return { phrase: m[2], value: m[1] };
  return null;
}

/** The settings the phrase names, with how well: every phrase word starts
 *  a word of the label or key, and the larger the share of the setting's
 *  own words the phrase covers, the better (an exact label wins). Best
 *  first. */
export function rankSettings(phrase: string, descs: Iterable<Descriptor>): { d: Descriptor; score: number }[] {
  const want = words(phrase);
  if (want.length === 0) return [];
  const scored: { d: Descriptor; score: number }[] = [];
  for (const d of descs) {
    const have = [...new Set([...words(d.label), ...words(d.key)])];
    const ok = want.every((w) => have.some((h) => h === w || (w.length >= 3 && h.startsWith(w))));
    if (!ok) continue;
    const exact = words(d.label).join(' ') === want.join(' ') ? 1 : 0;
    scored.push({ d, score: exact + want.length / have.length });
  }
  return scored.sort((a, b) => b.score - a.score);
}

/** `text` as the value `d` stores, or why it cannot be. */
export function parseValue(d: Descriptor, text: string): { value: string } | { error: string } {
  const t = text.trim().toLowerCase();
  switch (d.kind.type) {
    case 'bool':
      if (ON.has(t)) return { value: 'true' };
      if (OFF.has(t)) return { value: 'false' };
      return { error: 'say on or off' };
    case 'choice': {
      const hit = d.kind.options.find((o) => {
        const label = (d.option_labels?.find(([v]) => v === o)?.[1] ?? o).toLowerCase();
        return o.toLowerCase() === t || label === t;
      });
      return hit ? { value: hit } : { error: `one of: ${d.kind.options.join(', ')}` };
    }
    case 'choice_set': {
      const opts = d.kind.options;
      const parts = t.split(/\s*(?:,|\band\b)\s*/).filter(Boolean);
      const bad = parts.filter((p) => !opts.some((o) => o.toLowerCase() === p));
      if (bad.length) return { error: `not an option: ${bad.join(', ')}` };
      return { value: opts.filter((o) => parts.includes(o.toLowerCase())).join(',') };
    }
    case 'secs':
    case 'int': {
      const m = /^(\d+(?:\.\d+)?)\s*([a-z%]*)$/.exec(t);
      if (!m) return { error: 'say a number' };
      const [, num, unit] = m;
      let out: { value: string } | { error: string };
      const perUnit = SECS.find(([re]) => re.test(unit))?.[1];
      if (unit === '' || (d.kind.type === 'int' && !perUnit)) {
        out = fromDisplay(d, num);
      } else if (!perUnit) {
        return { error: `not a unit of time: ${unit}` };
      } else if (d.kind.type === 'secs') {
        out = { value: String(Math.round(Number(num) * perUnit)) };
      } else {
        // An int counted in a unit of time: 2 weeks of a days setting.
        const own = SECS_PER_UNIT[d.unit];
        if (!own) return { error: `${d.label} is not a time` };
        const n = (Number(num) * perUnit) / own;
        if (!Number.isInteger(n)) return { error: `a whole number of ${d.unit}` };
        out = { value: String(n) };
      }
      if ('error' in out) return out;
      const n = Number(out.value);
      const { min, max } = d.kind;
      if (n < min || n > max) return { error: `out of range` };
      return out;
    }
    case 'text':
      return text.trim().length <= d.kind.max ? { value: text.trim() } : { error: 'too long' };
    default:
      return { error: 'edit it on its page' };
  }
}

/** A command in plain words as one proposed change, or null when `query`
 *  is not a command. */
export function interpret(query: string, descs: Iterable<Descriptor>): NlResult | null {
  const cmd = splitCommand(query);
  if (!cmd) return null;
  const ranked = rankSettings(cmd.phrase, descs);
  if (ranked.length === 0) return { kind: 'error', message: `No setting is called “${cmd.phrase}”.` };
  // More than one match, and the words name less than half of the best
  // one (or two match as well): ask which, rather than guess.
  const tied = ranked.filter((r) => r.score === ranked[0].score);
  if (tied.length > 1 || (ranked.length > 1 && ranked[0].score < 0.5)) {
    return { kind: 'ambiguous', options: ranked.slice(0, 5).map((r) => r.d) };
  }
  const d = ranked[0].d;
  if (d.owned_by) return { kind: 'error', d, message: `${d.label} is read-only here: change it with ${d.owned_by}.` };
  const v = parseValue(d, cmd.value);
  if ('error' in v) return { kind: 'error', d, message: `${d.label}: ${v.error}.` };
  return { kind: 'change', d, value: v.value };
}
