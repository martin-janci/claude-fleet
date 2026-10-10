// What a guide changes (G7.15, Guide board): "4 steps · changes 2 settings"
// on its chat card, and while it is open, each setting it changed with the
// value before and an Undo ("Steps change real settings, and each change is
// listed so you can undo it"). Pure; PageView and GuidePageCard render.
import { sectionsOf, type Descriptor, type Page } from './pages';
import { valueInWords } from './review';

/** The settings a guide's steps can write, in step order, each once. A
 *  setting another page owns (`owned_by`) is shown there, not written here. */
export function guideFieldKeys(page: Page, descs: ReadonlyMap<string, Descriptor>): string[] {
  const keys: string[] = [];
  for (const { section } of sectionsOf(page)) {
    for (const item of section.items) {
      if (item.type !== 'field' || keys.includes(item.key)) continue;
      const d = descs.get(item.key);
      if (d && d.owned_by === undefined) keys.push(item.key);
    }
  }
  return keys;
}

/** The chat card's summary: "4 steps · changes 2 settings". */
export function guideSummary(page: Page, descs: ReadonlyMap<string, Descriptor>): string {
  const steps = sectionsOf(page).length;
  const n = guideFieldKeys(page, descs).length;
  const parts = [`${steps} ${steps === 1 ? 'step' : 'steps'}`];
  if (n > 0) parts.push(`changes ${n} ${n === 1 ? 'setting' : 'settings'}`);
  return parts.join(' · ');
}

/** One setting the guide changed since it opened. */
export interface GuideChange {
  key: string;
  label: string;
  /** The value when the guide opened: what Undo writes back. */
  before: string;
  /** Both values in words: "Off → On". */
  words: string;
}

/** The guide's settings whose value moved away from `before` (the values
 *  when the guide opened), in step order. Setting one back drops it. */
export function guideChanges(
  page: Page,
  descs: ReadonlyMap<string, Descriptor>,
  before: Readonly<Record<string, string>>,
  values: Readonly<Record<string, string>>,
): GuideChange[] {
  const out: GuideChange[] = [];
  for (const key of guideFieldKeys(page, descs)) {
    const d = descs.get(key)!;
    if (!Object.hasOwn(before, key)) continue;
    const was = before[key];
    const now = values[key] ?? d.value;
    if (now === was) continue;
    out.push({ key, label: d.label, before: was, words: `${valueInWords(d, was)} → ${valueInWords(d, now)}` });
  }
  return out;
}

/** The values a guide's settings hold now: `guideChanges`' `before`. */
export function guideSnapshot(
  page: Page,
  descs: ReadonlyMap<string, Descriptor>,
  values: Readonly<Record<string, string>>,
): Record<string, string> {
  return Object.fromEntries(guideFieldKeys(page, descs).map((k) => [k, values[k] ?? descs.get(k)!.value]));
}
