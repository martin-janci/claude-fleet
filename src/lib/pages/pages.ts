// Declarative pages (design docs/superpowers/specs/2026-09-28-declarative-pages-design.md,
// P3): the page specs compiled into the backend (`list_pages`), the settings
// registry's metadata (`describe_fleet_settings`), and the pure helpers the
// renderer uses — conditions, unit conversion, the page tree and search.
// Values themselves stay in `fleetSettings` (fleet_settings.ts), which every
// write already refreshes from the backend's answer.
import { writable, type Readable, derived } from 'svelte/store';
import { invokeCmd, type Result } from '../result';
import { fleetSettings, type FleetSettings } from '../fleet_settings';
import type { ResourceType } from './resources';

// ── the page DSL, as `crates/fleet-core/src/pages/model.rs` serialises it ──

export type Layout =
  | 'category'
  | 'master_detail'
  | 'flow'
  | 'object_editor'
  | 'cards'
  | 'review_apply'
  | 'data_page';

export type Widget =
  | 'switch'
  | 'number'
  | 'duration'
  | 'select'
  | 'radio'
  | 'multiselect'
  | 'text'
  | 'textarea'
  | 'key_value_table'
  | 'id_list'
  | 'readonly';

export interface Condition {
  key?: string;
  eq?: string;
  in?: string[];
  truthy?: boolean;
  all?: Condition[];
  any?: Condition[];
  not?: Condition;
}

export interface SourceRef {
  id: string;
  params?: Record<string, unknown>;
}

export type CustomComponent = 'work_retention' | 'auto_tidy_preview' | 'org_suggestions';

export type Item =
  | { type: 'field'; key: string; widget?: Widget; hint?: string; when?: Condition }
  | { type: 'stat'; source: SourceRef; field?: string; label?: string }
  | { type: 'record'; source: SourceRef }
  | { type: 'table'; source: SourceRef; columns?: string[] }
  | { type: 'chart'; source: SourceRef; chart: 'line' | 'bar' | 'stacked_bar' | 'sparkline'; title?: string }
  | { type: 'notice'; tone: 'info' | 'warn' | 'danger'; text: string }
  | { type: 'custom'; component: CustomComponent }
  | { type: 'link'; page: string; label?: string };

export interface Section {
  title: string;
  intro?: string;
  collapsible?: boolean;
  advanced?: boolean;
  when?: Condition;
  items: Item[];
}

export interface Tab {
  title: string;
  when?: Condition;
  sections: Section[];
}

export interface Page {
  spec: string;
  id: string;
  title: string;
  parent?: string;
  intro?: string;
  layout: Layout;
  /** A `master_detail` page's resource (`pages/resources.ts`). */
  resource?: string;
  /** A `master_detail` page's items about the whole list. */
  list_items?: Item[];
  sections?: Section[];
  tabs?: Tab[];
}

// ── data sources (`pages/sources.rs`) ──

export type ColType = 'text' | 'int' | 'tokens' | 'usd_micros' | 'day';

export interface Column {
  id: string;
  label: string;
  ty: ColType;
}

export type SourceShape =
  | { shape: 'scalar'; ty: ColType }
  | { shape: 'record'; fields: Column[] }
  | { shape: 'rows'; columns: Column[] }
  | { shape: 'series'; x: Column; y: Column[] };

export type SourceSpec = { id: string; label: string; help: string } & SourceShape;

export interface PagesBundle {
  pages: Page[];
  sources: SourceSpec[];
  resources: ResourceType[];
}

// ── the settings registry (`service/settings.rs` `Descriptor`) ──

export type KindDesc =
  | { type: 'bool' }
  | { type: 'secs'; min: number; max: number }
  | { type: 'int'; min: number; max: number }
  | { type: 'choice'; options: string[] }
  | { type: 'choice_set'; options: string[] }
  | { type: 'path_map' }
  | { type: 'id_set' }
  | { type: 'price_map' }
  | { type: 'text'; max: number };

export type Unit =
  | 'none'
  | 'ms'
  | 'seconds'
  | 'minutes'
  | 'hours'
  | 'days'
  | 'percent'
  | 'kib'
  | 'mib'
  | 'tokens'
  | 'count';

export interface Descriptor {
  key: string;
  label: string;
  help: string;
  kind: KindDesc;
  default: string;
  value: string;
  modified: boolean;
  unit: Unit;
  zero?: string;
  tags: ('advanced' | 'experimental' | 'network' | 'ai')[];
  danger: { level: 'none' } | { level: 'confirm'; message: string };
  restart: 'none' | 'app' | 'hooks';
  ai: 'suggest' | 'fill' | 'never';
  owned_by?: string;
  option_labels?: [string, string][];
}

// ── stores ──

export const pagesBundle = writable<PagesBundle>({ pages: [], sources: [], resources: [] });
export const descriptors = writable<Map<string, Descriptor>>(new Map());

export async function loadPages(): Promise<Result<PagesBundle>> {
  const r = await invokeCmd<PagesBundle>('list_pages');
  if (r.ok && r.value) pagesBundle.set(r.value);
  return r;
}

export async function loadDescriptors(): Promise<Result<Descriptor[]>> {
  const r = await invokeCmd<Descriptor[]>('describe_fleet_settings');
  if (r.ok && r.value) descriptors.set(new Map(r.value.map((d) => [d.key, d])));
  return r;
}

export function fetchSource(ref: SourceRef): Promise<Result<unknown>> {
  return invokeCmd<unknown>('fetch_page_source', { id: ref.id, params: ref.params ?? null });
}

/** The current value of every described setting: the live `fleetSettings`
 *  map (refreshed by every write) over the values `describe` returned. */
export const settingValues: Readable<Record<string, string>> = derived(
  [descriptors, fleetSettings],
  ([$d, $fs]) => valuesOf($d, $fs),
);

export function valuesOf(d: Map<string, Descriptor>, fs: FleetSettings): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, desc] of d) out[k] = fs[k] ?? desc.value;
  return out;
}

// ── conditions ──

/** Evaluate a `when` against current values. One form per condition, as
 *  the backend validator guarantees; a malformed one reads as shown. */
export function evalCondition(c: Condition | undefined, values: Record<string, string>): boolean {
  if (!c) return true;
  if (c.all) return c.all.every((x) => evalCondition(x, values));
  if (c.any) return c.any.some((x) => evalCondition(x, values));
  if (c.not) return !evalCondition(c.not, values);
  if (c.key === undefined) return true;
  const v = (values[c.key] ?? '').trim();
  if (c.eq !== undefined) return v === c.eq;
  if (c.in !== undefined) return c.in.includes(v);
  if (c.truthy !== undefined) return (v === 'true') === c.truthy;
  return true;
}

// ── units ──

const SECS_PER: Partial<Record<Unit, number>> = { seconds: 1, minutes: 60, hours: 3600, days: 86400 };

/** How many stored units make one shown unit: seconds shown in hours is
 *  3600; an Int already in its unit is 1. */
export function unitFactor(d: Descriptor): number {
  if (d.kind.type !== 'secs') return 1;
  return SECS_PER[d.unit] ?? 1;
}

/** The stored value as the number a person types. */
export function toDisplay(d: Descriptor, raw: string): string {
  const f = unitFactor(d);
  if (f === 1) return raw;
  const n = Number(raw);
  if (!Number.isFinite(n)) return raw;
  const shown = n / f;
  return Number.isInteger(shown) ? String(shown) : String(Math.round(shown * 100) / 100);
}

/** A typed number back to the stored text, or why it cannot be sent. The
 *  backend still owns the range check; this only refuses what is not a
 *  number, or a non-zero entry that would round to 0 (which means "off"). */
export function fromDisplay(d: Descriptor, typed: string): { value: string } | { error: string } {
  const t = typed.trim();
  if (t === '') return { error: 'enter a number' };
  const n = Number(t);
  if (!Number.isFinite(n) || n < 0) return { error: 'enter a number, 0 or more' };
  const f = unitFactor(d);
  if (f === 1) {
    if (!Number.isInteger(n)) return { error: 'enter a whole number' };
    return { value: String(n) };
  }
  const stored = Math.round(n * f);
  if (stored === 0 && n !== 0) return { error: `too small: under one second` };
  return { value: String(stored) };
}

export const UNIT_WORDS: Record<Unit, string> = {
  none: '',
  ms: 'ms',
  seconds: 'seconds',
  minutes: 'minutes',
  hours: 'hours',
  days: 'days',
  percent: '%',
  kib: 'KiB',
  mib: 'MiB',
  tokens: 'tokens',
  count: '',
};

/** "1–365 days", "0 = never", bounds in the shown unit, for the help line. */
export function rangeText(d: Descriptor): string {
  const u = UNIT_WORDS[d.unit];
  const f = unitFactor(d);
  let base = '';
  if (d.kind.type === 'int') base = `${d.kind.min}–${d.kind.max}${u ? ` ${u}` : ''}`;
  else if (d.kind.type === 'secs' && d.kind.min > 0)
    base = `at least ${d.kind.min / f}${u ? ` ${u}` : ''}`;
  else if (d.kind.type === 'secs') base = u;
  if (d.zero) base = base ? `${base}; 0 = ${d.zero}` : `0 = ${d.zero}`;
  return base;
}

export function optionLabel(d: Descriptor, value: string): string {
  return d.option_labels?.find(([v]) => v === value)?.[1] ?? value;
}

// ── the page tree and search ──

/** Pages in list order under `parent` (`null`: the top level). */
export function childrenOf(pages: Page[], parent: string | null): Page[] {
  return pages.filter((p) => (p.parent ?? null) === parent);
}

/** Every section of a page with the tab it sits in (`-1`: no tabs). */
export function sectionsOf(page: Page): { tab: number; section: Section }[] {
  if (page.tabs?.length) {
    return page.tabs.flatMap((t, i) => t.sections.map((section) => ({ tab: i, section })));
  }
  return (page.sections ?? []).map((section) => ({ tab: -1, section }));
}

/** Where a setting lives: the page and tab its `field` is on. */
export function homeOf(pages: Page[], key: string): { page: string; tab: number } | null {
  for (const p of pages) {
    for (const { tab, section } of sectionsOf(p)) {
      if (section.items.some((i) => i.type === 'field' && i.key === key)) return { page: p.id, tab };
    }
  }
  return null;
}

export interface SearchHit {
  key: string;
  label: string;
  page: string;
  pageTitle: string;
  tab: number;
}

/**
 * Settings matching `query` across every page: each word must appear in the
 * label, key, help or tags. `@modified` keeps only settings off their
 * default; `@tag:<tag>` only those with the tag. Hits are in page order.
 */
export function searchSettings(
  query: string,
  pages: Page[],
  descs: Map<string, Descriptor>,
  values: Record<string, string>,
): SearchHit[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  const hits: SearchHit[] = [];
  for (const p of pages) {
    for (const { tab, section } of sectionsOf(p)) {
      for (const item of section.items) {
        if (item.type !== 'field') continue;
        const d = descs.get(item.key);
        if (!d) continue;
        const hay = `${d.label} ${d.key} ${d.help} ${d.tags.join(' ')} ${section.title}`.toLowerCase();
        const ok = words.every((w) => {
          if (w === '@modified') return (values[d.key] ?? d.value) !== d.default;
          if (w.startsWith('@tag:')) return d.tags.includes(w.slice(5) as Descriptor['tags'][number]);
          return hay.includes(w);
        });
        if (ok) hits.push({ key: d.key, label: d.label, page: p.id, pageTitle: p.title, tab });
      }
    }
  }
  return hits;
}

// ── formatting data ──

export function formatCell(ty: ColType, v: unknown): string {
  if (v === null || v === undefined) return '—';
  switch (ty) {
    case 'usd_micros': {
      const usd = Number(v) / 1_000_000;
      return `$${usd >= 100 ? usd.toFixed(0) : usd.toFixed(2)}`;
    }
    case 'tokens': {
      const n = Number(v);
      if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
      if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
      return String(n);
    }
    case 'int':
      return Number(v).toLocaleString('en-US');
    default:
      return String(v);
  }
}
